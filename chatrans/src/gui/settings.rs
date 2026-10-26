//! 设置视图（S5 T03）：翻译配置的全部 UI 与本地编辑状态。
//!
//! 结构对齐设计文档第四节（图二五区映射）：每张供应商卡片 = 供应商名
//! + API Key（密文/明文切换）+ Base URL + 高级配置（超时/代理/自定义
//! 请求头）+ 模型区（搜索 / 获取列表 / 自定义添加 / 行开关）；内部编号
//! 自动生成、不在界面上露出（悬停"生效"可见）；
//! 卡片头单选"生效供应商"，顶部"保存配置"全量写盘。
//!
//! 数据流纪律（设计文档第五节）：
//!   · 编辑态是纯 UI 克隆（打开时从 store 快照），保存时才写回 ——
//!     未保存的改动绝不污染 worker 正在用的配置；
//!   · API Key 明文只在本结构体内存里，落盘永远走 store 的 DPAPI 路径；
//!   · 获取模型列表经 worker 线程异步执行（[`SettingsAction::FetchModels`]），
//!     结果由 app.rs 回灌 [`SettingsView::on_models_result`]，失败降级手动填。

use std::collections::{BTreeMap, HashSet};

use eframe::egui::{self, Align, Color32, DragValue, RichText, ScrollArea, TextEdit};

use crate::app_config::{is_valid_game_dir, AppConfig};
use crate::translate::store::{
    secret_key, EndpointModel, TranslationConfig, TranslationEndpoint, WireProtocol,
};

use super::app::{
    icon_button, ICON_DISABLED, ICON_ENABLED, TEXT_MAIN, TEXT_ORIGINAL, TEXT_STATUS, TEXT_WARN,
};
use super::theme::{button_fill, row_highlight, TEXT_ERROR, TEXT_OK};

// MDL2 / Fluent Icons 字形（设置视图专用；字体缺失时按钮自动退化文字占位）
const GLYPH_EYE_SHOW: char = '\u{E7B3}';
const GLYPH_EYE_HIDE: char = '\u{E898}';
const GLYPH_EDIT: char = '\u{E70F}';
const GLYPH_DELETE: char = '\u{E74D}';


/// 一张供应商卡片的本地编辑态。
#[derive(Debug, Clone)]
pub struct EditEndpoint {
    pub id: String,
    pub name: String,
    pub protocol: WireProtocol,
    pub base_url: String,
    pub model: String,
    pub enabled: bool,
    pub timeout_seconds: u64,
    pub proxy: String,
    /// 编辑态的请求头是行向量（UI 增删行），保存时折叠回 BTreeMap。
    pub headers: Vec<(String, String)>,
    pub models: Vec<EndpointModel>,
    /// 明文 key（只在本结构体内存）。
    pub key: String,
    // ── 纯 UI 状态（不落盘）───────────────────────────────────────────
    /// API Key 明文可见。
    pub show_key: bool,
    /// 模型搜索过滤串。
    pub filter: String,
    /// 自定义模型输入框内容。
    pub custom_model: String,
    /// 正在内联重命名的模型行下标。
    pub model_editing: Option<usize>,
    pub new_header_key: String,
    pub new_header_value: String,
}

impl EditEndpoint {
    fn from_config(
        ep: &TranslationEndpoint,
        store: &crate::translate::store::TranslationStore,
    ) -> EditEndpoint {
        EditEndpoint {
            id: ep.id.clone(),
            name: ep.name.clone(),
            protocol: ep.protocol,
            base_url: ep.base_url.clone(),
            model: ep.model.clone(),
            enabled: ep.enabled,
            timeout_seconds: ep.timeout_seconds,
            proxy: ep.proxy.clone(),
            headers: ep
                .extra_headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            models: ep.models.clone(),
            key: store.get_secret(&ep.secret_key()).unwrap_or("").to_string(),
            show_key: false,
            filter: String::new(),
            custom_model: String::new(),
            model_editing: None,
            new_header_key: String::new(),
            new_header_value: String::new(),
        }
    }

    /// 追加一行自定义模型（去重：同名不重复添加，返回是否真的加了）。
    pub fn add_custom_model(&mut self) -> bool {
        let id = self.custom_model.trim().to_string();
        if id.is_empty() || self.models.iter().any(|m| m.id == id) {
            return false;
        }
        self.models.push(EndpointModel { id, enabled: true });
        self.custom_model.clear();
        true
    }

    /// 删除模型行（越界静默忽略 —— 外部输入优雅降级的既定纪律）。
    pub fn remove_model(&mut self, index: usize) {
        if index < self.models.len() {
            self.models.remove(index);
        }
    }

    /// 模型搜索过滤：空串匹配全部。
    pub fn model_matches_filter(&self, index: usize) -> bool {
        match self.models.get(index) {
            Some(m) => self.filter.trim().is_empty() || m.id.contains(self.filter.trim()),
            None => false,
        }
    }

    /// 追加一行自定义请求头（空键忽略；同名覆盖旧行）。
    pub fn push_header(&mut self) {
        let key = self.new_header_key.trim().to_string();
        if key.is_empty() {
            return;
        }
        self.headers.retain(|(k, _)| *k != key);
        self.headers
            .push((key, self.new_header_value.trim().to_string()));
        self.new_header_key.clear();
        self.new_header_value.clear();
    }
}

/// 设置视图的整体编辑态（TranslationConfig + AppConfig 的 UI 镜像）。
#[derive(Debug, Clone)]
pub struct EditState {
    /// 游戏根目录（app.json 的 GameDir；独立于翻译配置）。
    pub game_dir: String,
    pub enabled: bool,
    pub target_language: String,
    pub team_only: bool,
    pub active_endpoint_id: String,
    pub endpoints: Vec<EditEndpoint>,
}

impl EditState {
    fn from_config(
        config: &TranslationConfig,
        app_config: &AppConfig,
        store: &crate::translate::store::TranslationStore,
    ) -> EditState {
        EditState {
            game_dir: app_config.game_dir.clone(),
            enabled: config.enabled,
            target_language: config.target_language.clone(),
            team_only: config.team_only,
            active_endpoint_id: config.active_endpoint_id.clone(),
            endpoints: config
                .endpoints
                .iter()
                .map(|ep| EditEndpoint::from_config(ep, store))
                .collect(),
        }
    }

    /// 追加一张新供应商卡片（uuid id），返回它的 id。
    pub fn add_endpoint(&mut self) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let name = self.next_default_name();
        self.endpoints.push(EditEndpoint {
            id: id.clone(),
            name,
            protocol: WireProtocol::OpenAi,
            base_url: String::new(),
            model: String::new(),
            enabled: true,
            timeout_seconds: 10,
            proxy: String::new(),
            headers: Vec::new(),
            models: Vec::new(),
            key: String::new(),
            show_key: false,
            filter: String::new(),
            custom_model: String::new(),
            model_editing: None,
            new_header_key: String::new(),
            new_header_value: String::new(),
        });
        // 首个供应商自动成为生效供应商（省一步单选）
        if self.active_endpoint_id.is_empty() {
            self.active_endpoint_id = id.clone();
        }
        id
    }

    /// 下一个默认名字：取最小的 unused 序号（删掉中间一张后再加，不会撞名）。
    fn next_default_name(&self) -> String {
        let mut n = 1;
        loop {
            let candidate = format!("新供应商 {}", n);
            if !self.endpoints.iter().any(|e| e.name == candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// 删除供应商卡片；若删的是生效供应商，生效位回落到剩余第一张（或空）。
    pub fn remove_endpoint(&mut self, index: usize) {
        if index >= self.endpoints.len() {
            return;
        }
        let removed = self.endpoints.remove(index);
        if self.active_endpoint_id == removed.id {
            self.active_endpoint_id = self.endpoints.first().map(|e| e.id.clone()).unwrap_or_default();
        }
    }

    /// 折叠成落盘快照。含裁决 2 的 Enabled→model 写回（首个 enabled 模型
    /// 写回 endpoint.model，全部关闭则原值不动 —— apply_enabled_model 语义）。
    pub fn into_snapshot(self) -> SettingsSnapshot {
        let mut secrets = Vec::new();
        let mut endpoints: Vec<TranslationEndpoint> = self
            .endpoints
            .into_iter()
            .map(|e| {
                secrets.push((secret_key(&e.id), e.key));
                let mut extra_headers = BTreeMap::new();
                for (k, v) in e.headers {
                    let k = k.trim().to_string();
                    if k.is_empty() {
                        continue;
                    }
                    extra_headers.insert(k, v.trim().to_string());
                }
                // 模型列表去重（保序，保留先出现的那行）
                let mut seen: HashSet<String> = HashSet::new();
                let models = e
                    .models
                    .into_iter()
                    .filter(|m| !m.id.trim().is_empty() && seen.insert(m.id.clone()))
                    .collect();
                TranslationEndpoint {
                    id: e.id,
                    name: e.name,
                    protocol: e.protocol,
                    base_url: e.base_url.trim().to_string(),
                    model: e.model,
                    enabled: e.enabled,
                    timeout_seconds: e.timeout_seconds,
                    proxy: e.proxy.trim().to_string(),
                    extra_headers,
                    models,
                }
            })
            .collect();

        for ep in &mut endpoints {
            ep.apply_enabled_model();
        }

        SettingsSnapshot {
            config: TranslationConfig {
                enabled: self.enabled,
                target_language: self.target_language.trim().to_string(),
                team_only: self.team_only,
                active_endpoint_id: self.active_endpoint_id,
                endpoints,
            },
            secrets,
            game_dir: self.game_dir.trim().to_string(),
        }
    }
}

/// 设置视图发给 app.rs 的动作（app.rs 负责执行：worker 往来 / 落盘 /
/// 管线决策 / rfd 对话框）。
pub enum SettingsAction {
    /// 拉取供应商模型列表。endpoint 是**编辑态**的当前值（未保存的改动也生效），
    /// key 是编辑框里的明文（可能尚未保存）。
    FetchModels {
        endpoint: TranslationEndpoint,
        key: String,
    },
    /// 游戏路径"浏览…"：后台线程弹 rfd 目录对话框（结果经 channel 回灌）。
    BrowseGameDir,
    /// 游戏路径"自动发现"：后台线程跑注册表 + 盘符扫描。
    DiscoverGameDir,
    /// 保存：编辑态写回 store + app_config 落盘 + 管线决策（app.rs 处理）。
    Save(SettingsSnapshot),
    /// 面板透明度实时预览（拖动滑块时每帧都会来）。`persist` = 这一下
    /// 要不要立刻落盘（拖动结束 / 键盘调整时为 true，拖动过程中为 false，
    /// 避免每帧写文件）。
    SetOpacity { percent: u8, persist: bool },
    /// 「仅显示翻译文字」开关：改一下立刻生效并落盘（勾选不是连续量，不用防抖）。
    SetOnlyTranslation { value: bool, persist: bool },
}

/// 保存时的完整快照：配置 + 每供应商的明文 key + 游戏路径编辑值
/// （app.rs 转手 set_secret / 写 app.json，落盘走 DPAPI / 原子写）。
pub struct SettingsSnapshot {
    pub config: TranslationConfig,
    /// `(store 密钥条目名, 明文)`；空串语义 = 删除该条目（store.set_secret）。
    pub secrets: Vec<(String, String)>,
    /// 游戏根目录编辑框的当前值（是否变化由 app.rs 对比 app_config 决定）。
    pub game_dir: String,
}

/// 设置视图。
pub struct SettingsView {
    open: bool,
    edit: EditState,
    /// 底部状态条：(文案, 是否错误)。
    status: Option<(String, bool)>,
    /// 正在拉取模型列表的供应商 id（按钮禁用 + 文案"获取中…"）。
    fetch_in_flight: HashSet<String>,
    /// 下一帧要滚动到视野里的供应商 id（"添加更多"后定位）。
    scroll_to: Option<String>,
    /// 面板不透明度（百分比）。实时预览：改一下立刻反映到窗口背景。
    opacity: u8,
    /// 「仅显示翻译文字」：勾选后聊天区只画译文。
    only_translation: bool,
}

impl Default for SettingsView {
    fn default() -> Self {
        SettingsView::new()
    }
}

// ── 版式（对齐设计图）──────────────────────────────────────────────────

/// 页面左右内边距。
const PAGE_PAD: i8 = 8;
/// 行内水平间距。
const GAP_X: f32 = 5.0;
/// 行与行之间的垂直间距。
const GAP_Y: f32 = 3.0;
/// 区块（游戏路径 / 卡片 / 显示区 / 底栏）之间的间距。
const SECTION_GAP: f32 = 4.0;
/// 输入框统一高度（DIP）。整页输入框一个高度，行与行才对得齐。
const FIELD_H: f32 = 22.0;
/// 左标签列宽度：API Key / Base URL / 模型 这些行靠它对齐。
const LABEL_COL_W: f32 = 62.0;
/// 低于这个**可用高度**（面板内区域，不含标题栏）就走"整页滚动"的小窗布局。
///
/// 三段式布局的固定开销：顶栏 34 + 游戏路径 62 + 显示区 32 + 底栏 44 ≈ 172px。
/// 留 90px 给中间列表（约三行）→ 262px 是分界；再矮就没有可用的卡片区了
/// （240 高的窗口实测只剩 21px，卡片等于看不见）。
const COMPACT_BELOW_H: f32 = 262.0;

/// 分区小标题（图里那种灰色小字，压在控件上方）。
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(1.0);
    ui.add(egui::Label::new(RichText::new(text).color(TEXT_STATUS).size(11.0)));
    ui.add_space(2.0);
}

/// 次级按钮：深灰填充 + 6px 圆角（浏览…／自动发现／获取模型列表…）。
fn secondary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(12.0))
        .fill(button_fill())
        .corner_radius(6)
}

/// 主按钮：比次级亮一档（保存配置）。
fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(12.0).color(Color32::WHITE))
        .fill(crate::gui::theme::BUTTON_PRIMARY_BG)
        .corner_radius(6)
}

/// 「左标签 + 右侧控件铺满剩余宽度」的一行。标签列固定宽，整页竖向对齐。
fn field_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        // 用 allocate_ui_with_layout 而不是 add_sized：后者会把标签居中，
        // 一列标签的左边缘就对不齐了。
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_COL_W, FIELD_H),
            egui::Layout::left_to_right(Align::Center),
            |ui| {
                ui.add(egui::Label::new(
                    RichText::new(label).color(TEXT_STATUS).size(11.0),
                ));
            },
        );
        add(ui);
    });
}

/// 铺满剩余宽度的单行输入框。
fn wide_text(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    let w = ui.available_width().max(80.0);
    ui.add_sized([w, FIELD_H], TextEdit::singleline(text).hint_text(hint))
}

/// 存储字段 `team_only` 与界面开关**方向相反**：
/// 勾选「翻译所有聊天」= 翻译全部频道 = `team_only == false`。
/// 两个函数成对使用，避免哪天有人在界面层直接反转布尔值把语义搞混。
fn translate_all_from(team_only: bool) -> bool {
    !team_only
}

fn team_only_from(translate_all: bool) -> bool {
    !translate_all
}

/// 语言代码 → 界面上的日常叫法（未知代码原样显示，不丢配置）。
fn language_label(code: &str) -> &str {
    match code {
        "zh" => "中文",
        "en" => "English",
        other => other,
    }
}

impl SettingsView {
    pub fn new() -> SettingsView {
        SettingsView {
            open: false,
            edit: EditState {
                game_dir: String::new(),
                enabled: false,
                target_language: "zh".to_string(),
                // 反相开关「翻译所有聊天」默认勾选 ⇒ team_only = false
                team_only: false,
                active_endpoint_id: String::new(),
                endpoints: Vec::new(),
            },
            status: None,
            fetch_in_flight: HashSet::new(),
            scroll_to: None,
            opacity: crate::gui::theme::OPACITY_DEFAULT,
            only_translation: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 同步面板当前不透明度（app 是唯一真源：落盘恢复/外部改动后推给视图）。
    pub fn set_opacity(&mut self, percent: u8) {
        self.opacity = percent;
    }

    /// 视图侧的不透明度（测试用）。
    pub fn opacity(&self) -> u8 {
        self.opacity
    }

    /// 同步「仅显示翻译文字」当前值（app 是真源：落盘恢复后推给视图）。
    pub fn set_only_translation(&mut self, on: bool) {
        self.only_translation = on;
    }

    /// 打开（从 store / app_config 快照编辑态）。重复调用会丢弃未保存的
    /// 编辑 —— 与"设置视图是纯 UI 克隆"的语义一致。
    pub fn open(
        &mut self,
        config: &TranslationConfig,
        app_config: &AppConfig,
        store: &crate::translate::store::TranslationStore,
    ) {
        self.edit = EditState::from_config(config, app_config, store);
        self.status = None;
        self.fetch_in_flight.clear();
        self.scroll_to = None;
        self.open = true;
    }

    /// 游戏路径回灌（rfd 选择/自动发现的后台结果）。给出人话状态提示，
    /// 路径无效时明确说明（不阻塞编辑 —— 用户可能还没拷完游戏）。
    pub fn set_game_dir(&mut self, dir: Option<String>) {
        let Some(dir) = dir else {
            return; // 用户取消：什么都不动
        };
        let valid = is_valid_game_dir(std::path::Path::new(dir.trim()));
        self.edit.game_dir = dir;
        self.status = Some((
            if valid {
                "游戏路径有效（保存后生效）".to_string()
            } else {
                "该目录不像游戏根目录（需要 WorldOfWarships.exe 或 replays/），仍可保存".to_string()
            },
            !valid,
        ));
    }

    /// 游戏路径是否与快照基线不同（app_config 比较用）。
    pub fn game_dir_changed(&self, app_config: &AppConfig) -> bool {
        self.edit.game_dir.trim() != app_config.game_dir.trim()
    }

    pub fn close(&mut self) {
        self.open = false;
        self.fetch_in_flight.clear();
        self.scroll_to = None;
    }

    /// 保存结果回灌（app.rs 落盘后调用）。
    pub fn on_saved(&mut self, ok: bool, message: &str) {
        self.status = Some((message.to_string(), !ok));
    }

    /// 标记供应商的模型列表拉取已在途。
    pub fn mark_fetch_in_flight(&mut self, endpoint_id: &str) {
        self.fetch_in_flight.insert(endpoint_id.to_string());
    }

    /// worker 结果回灌：成功把模型列表**去重合并**进编辑态（保留已有行的
    /// 开关状态，新 id 追加且默认启用）；失败置红字状态条 + 保留手动填。
    pub fn on_models_result(
        &mut self,
        endpoint_id: &str,
        models: Vec<String>,
        error: Option<String>,
    ) {
        self.fetch_in_flight.remove(endpoint_id);
        let Some(ep) = self.edit.endpoints.iter_mut().find(|e| e.id == endpoint_id) else {
            // 供应商在拉取期间被删了：结果丢弃即可
            return;
        };
        match error {
            Some(err) => {
                self.status = Some((format!("获取模型列表失败：{}（可手动填写）", err), true));
            }
            None => {
                let mut added = 0;
                for id in models {
                    let id = id.trim();
                    if id.is_empty() || ep.models.iter().any(|m| m.id == id) {
                        continue;
                    }
                    ep.models.push(EndpointModel {
                        id: id.to_string(),
                        enabled: true,
                    });
                    added += 1;
                }
                self.status = Some((format!("已合并 {} 个新模型", added), false));
            }
        }
    }

    // ── 渲染 ────────────────────────────────────────────────────────────

    /// 渲染整个设置视图，返回要执行的动作。设计裁决 6：打开期间整窗接管
    /// 由 app.rs 负责，本模块只管画。
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Vec<SettingsAction> {
        let mut actions = Vec::new();

        // 组件尺寸统一：按钮/复选框/下拉与输入框同高（22），内边距收紧。
        // 不做这一步，egui 默认按钮比输入框高一截，窄窗口里一眼就乱。
        // 间距全部是常量：窗口缩到最小也还是这套值，不做任何按比例的缩放。
        ui.spacing_mut().interact_size.y = FIELD_H;
        ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
        ui.spacing_mut().item_spacing = egui::vec2(GAP_X, GAP_Y);

        // 底部两块（显示设置 + 翻译范围）**任何窗口尺寸都钉死**。
        //
        // 之前小窗是"整页滚动"，底部跟着内容滚——结果窗口右下角既不好拖
        // （那一带全是滚动内容/滚动条），又常常只露出半行。现在底部永远
        // 固定，只有上面的内容滚。
        let compact = ui.available_height() < COMPACT_BELOW_H;

        egui::TopBottomPanel::bottom("settings_footer")
            .show_separator_line(false)
            .frame(
                egui::Frame::default()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(PAGE_PAD, 4)),
            )
            .show_inside(ui, |ui| self.draw_footer(ui));

        if compact {
            // 小窗：总开关/保存与游戏路径跟着卡片一起滚（底部保持钉住）。
            // 注意不能再套一层 ScrollArea —— 竖向滚动区里"可用高度"无界，
            // 内层 non-shrink 滚动区会撑到极大。
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::default()
                        .fill(Color32::TRANSPARENT)
                        .inner_margin(egui::Margin::symmetric(PAGE_PAD, 3)),
                )
                .show_inside(ui, |ui| {
                    ScrollArea::vertical()
                        .id_salt("settings_all")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.draw_header(ui, &mut actions);
                            self.draw_gamepath(ui, &mut actions);
                            ui.add_space(SECTION_GAP);
                            self.draw_cards_body(ui, &mut actions);
                        });
                });
            return actions;
        }

        // 常规：顶栏（总开关/保存）与游戏路径固定在上，底栏固定在下方，
        // 中间只滚供应商卡片。
        egui::TopBottomPanel::top("settings_header")
            .frame(
                egui::Frame::default()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(PAGE_PAD, 4)),
            )
            .show_inside(ui, |ui| self.draw_header(ui, &mut actions));

        egui::TopBottomPanel::top("settings_gamepath")
            .frame(
                egui::Frame::default()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(PAGE_PAD, 1)),
            )
            .show_inside(ui, |ui| self.draw_gamepath(ui, &mut actions));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(PAGE_PAD, 0)),
            )
            .show_inside(ui, |ui| self.draw_cards(ui, &mut actions));

        actions
    }

    /// 顶部：总开关 + 保存配置。
    fn draw_header(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.edit.enabled, "启用翻译");
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if ui.add(primary_button("保存配置")).clicked() {
                    actions.push(SettingsAction::Save(current_edit(&self.edit).into_snapshot()));
                }
            });
        });
    }

    /// 游戏路径：标签在上、输入框铺满、按钮贴右。
    fn draw_gamepath(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        section_label(ui, "游戏路径");
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 136.0).max(90.0);
            ui.add_sized(
                [w, FIELD_H],
                TextEdit::singleline(&mut self.edit.game_dir).hint_text(r"D:/Game/Korabli"),
            );
            if ui.add(secondary_button("浏览…")).clicked() {
                actions.push(SettingsAction::BrowseGameDir);
            }
            if ui.add(secondary_button("自动发现")).clicked() {
                actions.push(SettingsAction::DiscoverGameDir);
            }
        });
        ui.add_space(1.0);
        let valid = is_valid_game_dir(std::path::Path::new(self.edit.game_dir.trim()));
        let (text, color) = if self.edit.game_dir.trim().is_empty() {
            (
                "未配置：选游戏根目录（不是 replays），保存后自动开始监听对局",
                TEXT_WARN,
            )
        } else if valid {
            ("✔ 路径有效，保存后自动监听 replays 文件夹", TEXT_OK)
        } else {
            (
                "⚠ 目录无效（需要 WorldOfWarships.exe 或 replays/）",
                TEXT_WARN,
            )
        };
        // 窄窗口下截断，不要把窗口撑破
        ui.add(egui::Label::new(RichText::new(text).color(color).size(11.0)).truncate());
    }

    /// 底部：目标语言 / 频道范围 / 仅译文 / 透明度。
    fn draw_footer(&mut self, ui: &mut egui::Ui) {
        // 不再自己画分隔线：和上面的显示区合成一块，整块只有顶边一条线，
        // 小窗里不会出现"两行两条线"的碎感。
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("目标语言").color(TEXT_STATUS).size(11.0));
            // 语言用下拉说人话；老配置里的其它代码原样保留成一项。
            let current = self.edit.target_language.clone();
            egui::ComboBox::from_id_salt("target_language")
                .selected_text(RichText::new(language_label(&current)).size(12.0))
                .width(92.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.edit.target_language, "zh".to_string(), "中文");
                    ui.selectable_value(&mut self.edit.target_language, "en".to_string(), "English");
                    if !matches!(current.as_str(), "zh" | "en") {
                        ui.selectable_value(
                            &mut self.edit.target_language,
                            current.clone(),
                            format!("其他（{}）", current),
                        );
                    }
                });
            // 反相开关：勾选 = 翻译所有聊天（team_only=false）
            let mut translate_all = translate_all_from(self.edit.team_only);
            let resp = ui
                .checkbox(&mut translate_all, "翻译所有聊天")
                .on_hover_text("勾选：队伍频道和全体频道都翻译（全体聊天）\n取消勾选：只翻译队伍频道");
            if resp.changed() {
                self.edit.team_only = team_only_from(translate_all);
            }
        });
        if let Some((message, is_error)) = &self.status {
            let color = if *is_error { TEXT_ERROR } else { TEXT_OK };
            ui.add(egui::Label::new(RichText::new(message).color(color).size(11.0)).truncate());
        }
    }

    /// 显示设置（仅显示译文 / 面板透明度）：独立成一层，钉在底栏上方。
    ///
    /// 窄窗口（<300px）下这两项换成两行：一行放开关、一行放"标签 + 滑块"。
    /// 用 `horizontal_wrapped` 靠自动换行不靠谱 —— 滑块是定宽控件，宁可
    /// 溢出也不折行，会把 100% 顶到窗口外面去。
    fn draw_display(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        ui.separator();
        // 分隔线离下面的控件近一点（贴紧内容，别像悬在半空）
        ui.add_space(2.0);

        let wide = ui.available_width() >= 300.0;
        if wide {
            ui.horizontal(|ui| {
                self.display_checkbox(ui, actions);
                ui.add_space(8.0);
                self.opacity_slider(ui, actions);
            });
        } else {
            ui.horizontal(|ui| self.display_checkbox(ui, actions));
            ui.horizontal(|ui| self.opacity_slider(ui, actions));
        }
    }

    /// 「仅显示翻译文字」开关。
    fn display_checkbox(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        let mut only = self.only_translation;
        if ui
            .checkbox(&mut only, "仅显示翻译文字")
            .on_hover_text("勾选：每条只显示译文，不再显示原文小字\n取消：译文和原文都显示")
            .changed()
        {
            self.only_translation = only;
            actions.push(SettingsAction::SetOnlyTranslation {
                value: only,
                persist: true,
            });
        }
    }

    /// 「面板透明度」标签 + 滑块。滑块吃掉本行剩余宽度。
    fn opacity_slider(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        // 预留 = 数值框 44 + 间距 8：剩下的都给滑块轨道
        ui.spacing_mut().slider_width = (ui.available_width() - 100.0).clamp(60.0, 420.0);
        ui.label(RichText::new("面板透明度").color(TEXT_STATUS).size(11.0));
        let slider = ui.add(
            egui::Slider::new(
                &mut self.opacity,
                crate::gui::theme::OPACITY_MIN..=crate::gui::theme::OPACITY_MAX,
            )
            .suffix("%")
            .fixed_decimals(0),
        );
        let slider =
            slider.on_hover_text("只调背景：文字和图标始终完全不透明。拖动即时生效，松手后记住。");
        if slider.changed() {
            actions.push(SettingsAction::SetOpacity {
                percent: self.opacity,
                persist: false,
            });
        }
        if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
            actions.push(SettingsAction::SetOpacity {
                percent: self.opacity,
                persist: true,
            });
        }
    }

    /// 供应商卡片列表（常规布局：只滚这部分）。
    fn draw_cards(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("settings_scroll")
            .show(ui, |ui| self.draw_cards_body(ui, actions));
    }

    /// 卡片本体（不含滚动容器）：常规/小窗两种布局共用。
    fn draw_cards_body(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
                for index in 0..self.edit.endpoints.len() {
                    let Some((id, card_actions)) = self.render_endpoint_card(ui, index) else {
                        // 卡片被删除：本轮到此为止，下一帧重画
                        break;
                    };
                    if Some(&id) == self.scroll_to.as_ref() {
                        ui.scroll_to_cursor(Some(Align::TOP));
                        self.scroll_to = None;
                    }
                    actions.extend(card_actions);
                }
                ui.add_space(SECTION_GAP);
                // 无框文字按钮：层级更轻，不和"保存配置"抢视线
                let add = egui::Button::new(
                    RichText::new("＋ 添加更多模型供应商")
                        .color(TEXT_STATUS)
                        .size(12.0),
                )
                .fill(Color32::TRANSPARENT)
                .stroke(egui::Stroke::NONE)
                .corner_radius(6);
                if ui.add(add).clicked() {
                    let id = self.edit.add_endpoint();
                    self.scroll_to = Some(id);
                }
                ui.add_space(SECTION_GAP);
                // 显示设置跟着卡片一起滚（只有底栏"目标语言/翻译所有聊天"钉住）
                self.draw_display(ui, actions);
                ui.add_space(SECTION_GAP);
    }

    /// 渲染一张供应商卡片。返回 `(供应商 id, 动作)`；卡片被删除时返回 None
    /// （调用方本轮直接收尾，下一帧重画）。
    ///
    /// 借用策略：把卡片从编辑态临时取出来（placeholder 占位）在本地
    /// `ep` 上渲染，闭包只捕获本地变量与 `self.edit.active_endpoint_id`
    /// 这一个不相交字段；删除动作经 flag 在闭包外执行。
    fn render_endpoint_card(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
    ) -> Option<(String, Vec<SettingsAction>)> {
        let mut actions = Vec::new();
        let in_flight = self.fetch_in_flight.contains(&self.edit.endpoints[index].id);
        let mut ep = std::mem::replace(&mut self.edit.endpoints[index], EditEndpoint::placeholder());
        let id = ep.id.clone();
        let mut delete_requested = false;

        ui.add_space(SECTION_GAP);
        ui.separator();
        ui.add_space(SECTION_GAP);

        // 卡片头：[生效单选] [供应商名 铺满] [启用] [删除]
        ui.horizontal(|ui| {
            // 内部编号不再占一行：悬停在"生效"上才露出来，排查时够用。
            ui.radio_value(&mut self.edit.active_endpoint_id, id.clone(), "生效")
                .on_hover_text(format!(
                    "内部编号：{}\n自动生成，仅用于本机密钥存储，不需要你填",
                    ep.id
                ));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, GLYPH_DELETE, "删", "删除此供应商", ICON_ENABLED) {
                    delete_requested = true;
                }
                ui.checkbox(&mut ep.enabled, "启用");
                let w = (ui.available_width() - 4.0).max(80.0);
                ui.add_sized(
                    [w, FIELD_H],
                    TextEdit::singleline(&mut ep.name).hint_text("供应商名"),
                );
            });
        });

        if !delete_requested {
            // API Key：标签在左、输入框铺满、眼睛图标贴右（图里的顺序）
            field_row(ui, "API Key", |ui| {
                let w = (ui.available_width() - 28.0).max(80.0);
                ui.add_sized(
                    [w, FIELD_H],
                    TextEdit::singleline(&mut ep.key)
                        .password(!ep.show_key)
                        .hint_text("sk-…"),
                );
                let eye_glyph = if ep.show_key { GLYPH_EYE_HIDE } else { GLYPH_EYE_SHOW };
                let eye_fallback = if ep.show_key { "隐" } else { "显" };
                let show = ep.show_key;
                if icon_button(ui, eye_glyph, eye_fallback, "显示/隐藏密钥", ICON_ENABLED) {
                    ep.show_key = !show;
                }
            });

            field_row(ui, "Base URL", |ui| {
                wide_text(ui, &mut ep.base_url, "https://api.deepseek.com");
            });

            // 高级配置
            egui::CollapsingHeader::new(RichText::new("高级配置").color(TEXT_STATUS).size(11.0))
                .id_salt(format!("adv-{}", ep.id))
                .default_open(false)
                .show(ui, |ui| {
                    field_row(ui, "超时", |ui| {
                        ui.add(
                            DragValue::new(&mut ep.timeout_seconds)
                                .range(3..=120)
                                .suffix(" 秒"),
                        );
                    });
                    field_row(ui, "代理", |ui| {
                        wide_text(ui, &mut ep.proxy, "http://127.0.0.1:7890");
                    });
                    section_label(ui, "自定义请求头");
                    let mut remove_index: Option<usize> = None;
                    for (row, (key, value)) in ep.headers.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let w = ((ui.available_width() - 150.0) / 2.0).max(70.0);
                            ui.add_sized([w, FIELD_H], TextEdit::singleline(key).hint_text("名称"));
                            ui.add_sized([w, FIELD_H], TextEdit::singleline(value).hint_text("值"));
                            if icon_button(ui, GLYPH_DELETE, "删", "删除此行", ICON_ENABLED) {
                                remove_index = Some(row);
                            }
                        });
                    }
                    if let Some(row) = remove_index {
                        if row < ep.headers.len() {
                            ep.headers.remove(row);
                        }
                    }
                    ui.horizontal(|ui| {
                        let w = ((ui.available_width() - 96.0) / 2.0).max(70.0);
                        ui.add_sized(
                            [w, FIELD_H],
                            TextEdit::singleline(&mut ep.new_header_key).hint_text("新名称"),
                        );
                        ui.add_sized(
                            [w, FIELD_H],
                            TextEdit::singleline(&mut ep.new_header_value).hint_text("值"),
                        );
                        if ui.add(secondary_button("＋")).clicked() {
                            ep.push_header();
                        }
                    });
                });

            // 模型区（自由函数：闭包借用只落在本地 ep 上）
            actions.extend(render_model_section(ui, &mut ep, in_flight));
        }

        if delete_requested {
            self.edit.remove_endpoint(index);
            return None;
        }
        self.edit.endpoints[index] = ep;
        Some((id, actions))
    }
}

impl EditEndpoint {
    /// 占位卡片（render_endpoint_card 临时取出的空位；不会出现在 UI 上）。
    fn placeholder() -> EditEndpoint {
        EditEndpoint {
            id: String::new(),
            name: String::new(),
            protocol: WireProtocol::OpenAi,
            base_url: String::new(),
            model: String::new(),
            enabled: false,
            timeout_seconds: 10,
            proxy: String::new(),
            headers: Vec::new(),
            models: Vec::new(),
            key: String::new(),
            show_key: false,
            filter: String::new(),
            custom_model: String::new(),
            model_editing: None,
            new_header_key: String::new(),
            new_header_value: String::new(),
        }
    }
}

/// 模型区：搜索 / 获取列表 / 自定义添加 / 模型行（开关+名称+编辑+删除）。
/// 自由函数 —— 只碰本地 `ep`，避免与外层 self 借用打架。
fn render_model_section(
    ui: &mut egui::Ui,
    ep: &mut EditEndpoint,
    in_flight: bool,
) -> Vec<SettingsAction> {
    let mut actions = Vec::new();

    // FetchModels 动作要的数据（编辑态当前值，未保存的改动也生效）
    let mut endpoint_for_fetch = TranslationEndpoint {
        id: ep.id.clone(),
        name: ep.name.clone(),
        protocol: ep.protocol,
        base_url: ep.base_url.clone(),
        model: ep.model.clone(),
        enabled: ep.enabled,
        timeout_seconds: ep.timeout_seconds,
        proxy: ep.proxy.clone(),
        extra_headers: ep
            .headers
            .iter()
            .filter(|(k, _)| !k.trim().is_empty())
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect(),
        models: ep.models.clone(),
    };
    endpoint_for_fetch.apply_enabled_model();
    let key_for_fetch = ep.key.clone();

    ui.add_space(SECTION_GAP);
    ui.separator();
    ui.add_space(SECTION_GAP);

    // 搜索 + 拉取：搜索框铺满，按钮贴右
    field_row(ui, "模型", |ui| {
        let w = (ui.available_width() - 108.0).max(70.0);
        ui.add_sized(
            [w, FIELD_H],
            TextEdit::singleline(&mut ep.filter).hint_text("搜索模型…"),
        );
        let label = if in_flight { "获取中…" } else { "获取模型列表" };
        let fetch = ui.add_enabled(!in_flight, secondary_button(label));
        if fetch.clicked() {
            actions.push(SettingsAction::FetchModels {
                endpoint: endpoint_for_fetch,
                key: key_for_fetch,
            });
        }
    });
    if in_flight {
        ui.add(
            egui::Label::new(
                RichText::new("正在从供应商拉取模型列表…")
                    .color(TEXT_WARN)
                    .size(11.0),
            )
            .truncate(),
        );
    }

    // 自定义模型：与上面同一列起跑（空标签占位），按钮贴右
    field_row(ui, "", |ui| {
        let w = (ui.available_width() - 62.0).max(70.0);
        ui.add_sized(
            [w, FIELD_H],
            TextEdit::singleline(&mut ep.custom_model).hint_text("自定义模型 ID"),
        );
        if ui.add(secondary_button("添加")).clicked() {
            ep.add_custom_model();
        }
    });

    ui.add_space(1.0);

    // 模型行：名称占满，改/删图标贴右（图里的排布）
    let count = ep.models.len();
    let first_enabled = ep.models.iter().position(|m| m.enabled);
    for row in 0..count {
        if !ep.model_matches_filter(row) {
            continue;
        }
        let highlight = Some(row) == first_enabled;
        let editing = ep.model_editing == Some(row);
        let name = ep.models[row].id.clone();
        egui::Frame::default()
            .fill(if highlight {
                row_highlight()
            } else {
                Color32::TRANSPARENT
            })
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut ep.models[row].enabled, "");
                    // 右侧先占位（RTL）：删除 → 重命名，然后剩下的宽度全给名称。
                    // 用 RTL 子布局是因为它能拿到"整行剩余宽度"；直接在横向
                    // 布局里算 available_width 会拿到被压缩后的值。
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, GLYPH_DELETE, "删", "删除模型", ICON_ENABLED) {
                            ep.remove_model(row);
                        }
                        if icon_button(ui, GLYPH_EDIT, "改", "重命名模型", ICON_DISABLED) {
                            ep.model_editing = Some(row);
                        }
                        let w = ui.available_width().max(60.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(w, FIELD_H),
                            egui::Layout::left_to_right(Align::Center),
                            |ui| {
                                if editing {
                                    let w = ui.available_width();
                                    ui.add_sized(
                                        [w, FIELD_H],
                                        TextEdit::singleline(&mut ep.models[row].id),
                                    );
                                } else {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&name).size(12.0).color(if highlight {
                                                TEXT_MAIN
                                            } else {
                                                TEXT_ORIGINAL
                                            }),
                                        )
                                        .truncate(),
                                    );
                                }
                            },
                        );
                    });
                });
            });
    }
    if count == 0 {
        // 空态：不再把内部字段名（endpoint.model）甩给用户；有值就直接报出来。
        let text = if ep.model.trim().is_empty() {
            "还没有模型：点「获取模型列表」，或手动添加。".to_string()
        } else {
            format!(
                "还没有模型：点「获取模型列表」，或手动添加。当前使用的模型：{}",
                ep.model.trim()
            )
        };
        ui.add(
            egui::Label::new(RichText::new(text).color(TEXT_ORIGINAL).size(11.0)).truncate(),
        );
    }
    actions
}

/// 深拷贝编辑态（保存动作要消费快照，但编辑态必须保留 —— 保存失败可继续改）。
fn current_edit(state: &EditState) -> EditState {
    state.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::store::TranslationStore;
    

    /// 反相开关：默认勾选（翻译所有聊天），勾/取消与 team_only 一一对应。
    #[test]
    fn translate_all_switch_is_inverted() {
        // 默认：界面勾选 = 翻译所有频道
        assert!(translate_all_from(false), "team_only=false → 开关勾选");
        assert!(!translate_all_from(true), "team_only=true → 开关不勾");

        // 往返不丢语义
        for team_only in [true, false] {
            assert_eq!(team_only_from(translate_all_from(team_only)), team_only);
        }

        // 新建配置的默认值就是「翻译所有聊天」
        assert!(!TranslationConfig::default().team_only, "默认应翻译所有频道");
        assert!(!SettingsView::new().edit.team_only, "编辑态默认同样");
    }

    /// 语言代码在界面上要说人话；不认识的代码原样显示（不吞配置）。
    #[test]
    fn language_labels_are_plain_words() {
        assert_eq!(language_label("zh"), "中文");
        assert_eq!(language_label("en"), "English");
        assert_eq!(language_label("ja"), "ja");
        assert_eq!(language_label(""), "");
    }

    fn temp_store(tag: &str) -> TranslationStore {
        let dir = std::env::temp_dir()
            .join(format!("chatrans-settings-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TranslationStore::new_in(dir)
    }

    fn sample_config() -> TranslationConfig {
        let mut ep1 = TranslationEndpoint {
            id: "ep1".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            model: "deepseek-chat".to_string(),
            ..Default::default()
        };
        ep1.extra_headers.insert("X-Foo".to_string(), "bar".to_string());
        ep1.models = vec![
            EndpointModel { id: "deepseek-chat".to_string(), enabled: true },
            EndpointModel { id: "deepseek-reasoner".to_string(), enabled: false },
        ];
        let ep2 = TranslationEndpoint {
            id: "ep2".to_string(),
            name: "Anthropic".to_string(),
            protocol: WireProtocol::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-3-haiku".to_string(),
            ..Default::default()
        };
        TranslationConfig {
            enabled: true,
            target_language: "zh".to_string(),
            team_only: false,
            active_endpoint_id: "ep2".to_string(),
            endpoints: vec![ep1, ep2],
        }
    }

    fn json_of(config: &TranslationConfig) -> serde_json::Value {
        serde_json::to_value(config).expect("serialize")
    }

    /// 编辑态 round-trip：打开 → 快照，配置一字不丢；Enabled→model 写回
    /// 语义生效（首个 enabled 模型 == model 时不动，改了写回）。
    #[test]
    fn edit_snapshot_round_trip() {
        let store = temp_store("round-trip");
        let config = sample_config();
        let mut view = SettingsView::new();
        view.open(&config, &AppConfig::default(), &store);
        assert!(view.is_open());

        let snapshot = current_edit(&view.edit).into_snapshot();
        assert_eq!(json_of(&snapshot.config), json_of(&config), "round-trip 不丢字段");
        // 密钥条目名：ep1/ep2 两条（此刻 key 都是空串 = 删除语义，正常）
        let refs: Vec<&str> = snapshot.secrets.iter().map(|(k, _)| k.as_str()).collect();
        assert!(refs.contains(&"ep:ep1"));
        assert!(refs.contains(&"ep:ep2"));
    }

    /// Enabled→model 写回：编辑态里把开关换人，保存时 model 跟着走。
    #[test]
    fn snapshot_writeback_first_enabled_model() {
        let store = temp_store("writeback");
        let mut view = SettingsView::new();
        view.open(&sample_config(), &AppConfig::default(), &store);
        // ep1：deepseek-chat 关掉，reasoner 打开
        view.edit.endpoints[0].models[0].enabled = false;
        view.edit.endpoints[0].models[1].enabled = true;

        let snapshot = current_edit(&view.edit).into_snapshot();
        assert_eq!(snapshot.config.endpoints[0].model, "deepseek-reasoner");
        // ep2 无模型列表：model 原值不动
        assert_eq!(snapshot.config.endpoints[1].model, "claude-3-haiku");
    }

    /// 添加/删除供应商卡片：uuid id、首个自动生效、删生效供应商回落。
    #[test]
    fn add_and_remove_endpoints() {
        let mut edit = EditState {
            game_dir: String::new(),
            enabled: true,
            target_language: "zh".to_string(),
            team_only: true,
            active_endpoint_id: String::new(),
            endpoints: Vec::new(),
        };
        let id1 = edit.add_endpoint();
        assert_eq!(edit.active_endpoint_id, id1, "首个供应商自动生效");
        let id2 = edit.add_endpoint();
        assert_ne!(id1, id2, "uuid id 不重复");
        assert_eq!(edit.endpoints.len(), 2);

        edit.remove_endpoint(0); // 删掉生效供应商 → 回落到剩余第一张
        assert_eq!(edit.endpoints.len(), 1);
        assert_eq!(edit.active_endpoint_id, id2);
        edit.remove_endpoint(99); // 越界静默
        assert_eq!(edit.endpoints.len(), 1);
    }

    /// 默认名字自动编号：删掉中间一张后再加，补空位而不是一直往上加。
    /// （id 是 uuid，界面上看不到，名字是用户区分卡片的唯一线索，不能重名。）
    #[test]
    fn default_names_fill_the_gap() {
        let mut edit = EditState {
            game_dir: String::new(),
            enabled: true,
            target_language: "zh".to_string(),
            team_only: true,
            active_endpoint_id: String::new(),
            endpoints: Vec::new(),
        };
        edit.add_endpoint();
        edit.add_endpoint();
        edit.add_endpoint();
        let names: Vec<_> = edit.endpoints.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["新供应商 1", "新供应商 2", "新供应商 3"]);

        edit.remove_endpoint(1); // 删掉 2
        edit.add_endpoint();
        assert_eq!(edit.endpoints.last().unwrap().name, "新供应商 2", "补 2 的空位");
        assert_eq!(edit.endpoints.len(), 3);
    }

    /// on_models_result：去重合并进编辑态，保留已有行开关；失败保留手动填。
    #[test]
    fn models_result_merge_and_error() {
        let store = temp_store("merge");
        let mut view = SettingsView::new();
        view.open(&sample_config(), &AppConfig::default(), &store);
        view.edit.endpoints[0].models[0].enabled = false; // deepseek-chat 手动关掉过
        view.mark_fetch_in_flight("ep1");

        view.on_models_result(
            "ep1",
            vec![
                "deepseek-chat".to_string(),   // 已存在：跳过（不重置开关）
                "deepseek-reasoner".to_string(), // 已存在：跳过
                "deepseek-v3".to_string(),     // 新：追加且默认启用
                "  ".to_string(),              // 空白：丢弃
            ],
            None,
        );
        assert!(!view.fetch_in_flight.contains("ep1"), "结果到了要解除在途标记");
        let models = &view.edit.endpoints[0].models;
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "deepseek-chat");
        assert_eq!(models[0].enabled, false, "已有行开关不被合并覆盖");
        assert_eq!(models[1].enabled, false, "sample 里 reasoner 本来就是关闭的");
        assert_eq!(models[2].id, "deepseek-v3");
        assert_eq!(models[2].enabled, true, "新追加的默认启用");

        // 失败：红字状态 + 手动模型原样保留
        view.mark_fetch_in_flight("ep1");
        view.on_models_result("ep1", Vec::new(), Some("HTTP 401 Unauthorized".to_string()));
        assert!(matches!(&view.status, Some((m, true)) if m.contains("401")));
        assert_eq!(view.edit.endpoints[0].models.len(), 3, "失败不清空手动填的模型");
        // 供应商不存在：静默丢弃
        view.on_models_result("no-such", vec!["x".to_string()], None);
    }

    /// 自定义模型添加（去重）、删除行（越界安全）、搜索过滤。
    #[test]
    fn custom_model_and_filter() {
        let store = temp_store("custom");
        let mut view = SettingsView::new();
        view.open(&sample_config(), &AppConfig::default(), &store);
        let ep = &mut view.edit.endpoints[0];

        assert!(!ep.add_custom_model(), "空输入不加");
        ep.custom_model = "deepseek-chat".to_string();
        assert!(!ep.add_custom_model(), "同名去重");
        ep.custom_model = "  my-model  ".to_string();
        assert!(ep.add_custom_model(), "合法输入加入并清洗空白");
        assert_eq!(ep.models.last().unwrap().id, "my-model");
        assert!(ep.custom_model.is_empty(), "添加后清空输入框");

        ep.remove_model(999); // 越界静默
        let before = ep.models.len();
        ep.remove_model(0);
        assert_eq!(ep.models.len(), before - 1);
    }

    /// 自定义请求头：加行（同名覆盖）、折叠进 BTreeMap。
    #[test]
    fn header_rows_fold_into_map() {
        let store = temp_store("headers");
        let mut view = SettingsView::new();
        view.open(&sample_config(), &AppConfig::default(), &store);
        let ep = &mut view.edit.endpoints[0];
        assert_eq!(ep.headers.len(), 1); // X-Foo 来自 sample_config
        ep.new_header_key = "X-Bar".to_string();
        ep.new_header_value = "42".to_string();
        ep.push_header();
        assert_eq!(ep.headers.len(), 2);
        // 同名覆盖
        ep.new_header_key = " X-Foo ".to_string();
        ep.new_header_value = "baz".to_string();
        ep.push_header();
        assert_eq!(ep.headers.len(), 2, "同名行被覆盖而不是追加");
        // 空键忽略
        ep.new_header_key = "  ".to_string();
        ep.push_header();
        assert_eq!(ep.headers.len(), 2);

        let snapshot = current_edit(&view.edit).into_snapshot();
        let headers = &snapshot.config.endpoints[0].extra_headers;
        assert_eq!(headers.get("X-Foo").map(String::as_str), Some("baz"));
        assert_eq!(headers.get("X-Bar").map(String::as_str), Some("42"));
    }

    /// 打开 = 从 store 快照；关闭 = 清 UI 状态；重复打开丢弃未保存编辑。
    #[test]
    fn open_close_semantics() {
        let store = temp_store("open-close");
        let mut view = SettingsView::new();
        assert!(!view.is_open());
        view.open(&sample_config(), &AppConfig::default(), &store);
        assert!(view.is_open());
        // 未保存的编辑
        view.edit.endpoints[0].name = "改了一半".to_string();
        view.mark_fetch_in_flight("ep1");
        view.close();
        assert!(!view.is_open());
        // 重新打开：快照恢复原值
        view.open(&sample_config(), &AppConfig::default(), &store);
        assert_eq!(view.edit.endpoints[0].name, "DeepSeek");
        assert!(view.fetch_in_flight.is_empty(), "关闭时清在途标记");
    }

    /// 模型搜索过滤：空串全过、子串匹配、越界 false。
    #[test]
    fn model_filter_rules() {
        let store = temp_store("filter");
        let mut view = SettingsView::new();
        view.open(&sample_config(), &AppConfig::default(), &store);
        let ep = &mut view.edit.endpoints[0];
        assert!(ep.model_matches_filter(0), "空过滤全过");
        assert!(ep.model_matches_filter(1));
        ep.filter = "reasoner".to_string();
        assert!(!ep.model_matches_filter(0));
        assert!(ep.model_matches_filter(1));
        assert!(!ep.model_matches_filter(99), "越界 false");
        ep.filter = "  ".to_string(); // 纯空白等同空
        assert!(ep.model_matches_filter(0));
    }

    /// 快照清洗：base_url/proxy 去空白、空键头丢弃、空白模型 id 丢弃。
    #[test]
    fn snapshot_trims_and_cleans() {        let store = temp_store("clean");
        let mut view = SettingsView::new();
        view.open(&sample_config(), &AppConfig::default(), &store);
        {
            let ep = &mut view.edit.endpoints[0];
            ep.base_url = "  https://x.com/  ".to_string();
            ep.proxy = " http://127.0.0.1:7890 ".to_string();
            ep.headers.push(("".to_string(), "v".to_string()));
            ep.headers.push(("  ".to_string(), "v".to_string()));
            ep.models.push(EndpointModel { id: "  ".to_string(), enabled: true });
        }
        let snapshot = current_edit(&view.edit).into_snapshot();
        let ep = &snapshot.config.endpoints[0];
        // 快照只做空白清洗；尾斜杠留给 translate 链路的 resolve 处理
        assert_eq!(ep.base_url, "https://x.com/");
        assert_eq!(ep.proxy, "http://127.0.0.1:7890");
        assert_eq!(ep.extra_headers.len(), 1, "空键头被丢弃");
        assert_eq!(ep.models.len(), 2, "空白模型 id 被丢弃");
        let _ = std::fs::remove_dir_all(store.data_directory());
    }

    /// 游戏路径回灌：有效/无效给不同提示，取消（None）不动编辑态。
    #[test]
    fn set_game_dir_semantics() {
        let store = temp_store("gamedir");
        let mut view = SettingsView::new();
        let app_config = AppConfig::default();
        view.open(&sample_config(), &app_config, &store);
        assert!(view.edit.game_dir.is_empty());

        // 取消：编辑态不动
        view.set_game_dir(None);
        assert!(view.edit.game_dir.is_empty());

        // 有效路径（临时目录 + replays/）
        let good = temp_dir_for_game();
        std::fs::create_dir_all(good.join("replays")).unwrap();
        view.set_game_dir(Some(good.to_string_lossy().to_string()));
        assert!(matches!(&view.status, Some((m, false)) if m.contains("有效")));
        assert_eq!(view.edit.game_dir, good.to_string_lossy().to_string());

        // 无效路径：警告但不阻塞
        view.set_game_dir(Some(r"Q:\no\such".to_string()));
        assert!(
            matches!(&view.status, Some((m, true)) if m.contains("不像游戏根目录")),
            "无效路径要有人话警告: {:?}",
            view.status
        );
        assert_eq!(view.edit.game_dir, r"Q:\no\such");

        // 快照携带 game_dir；变化检测
        assert!(view.game_dir_changed(&app_config));
        let snapshot = current_edit(&view.edit).into_snapshot();
        assert_eq!(snapshot.game_dir, r"Q:\no\such");
        let mut same = AppConfig::default();
        same.game_dir = r"Q:\no\such".to_string();
        // 重新打开以重建 edit（into_snapshot 已消费）
        view.open(&sample_config(), &same, &store);
        assert!(!view.game_dir_changed(&same), "与 app_config 相同 → 未变化");
    }

    fn temp_dir_for_game() -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("chatrans-settings-game-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
