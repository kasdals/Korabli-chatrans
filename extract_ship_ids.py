"""从解包后的游戏数据提取 index -> ShipParamsId 映射。

数据来源：<游戏目录>/res_unpack/content/split/<版本>/Ship/*.json
- 文件名前缀是 index（如 PASA002_Bogue_1942.json -> PASA002）
- 文件顶层 `"id"` 就是玩家列表里出现的 ShipParamsId

只读文件、按行匹配顶层 id（缩进 4 空格），不解析整份 JSON，速度快很多。
"""

import os
import re
import sys
import glob

ship_dir = sys.argv[1]
write_to = sys.argv[2] if len(sys.argv) > 2 else None

INDEX_RE = re.compile(r'^P[A-Z]{2}[A-Z]\d{3}$')
ID_RE = re.compile(r'^    "id": (\d+)', re.M)

rows = []
bad = []
for path in glob.glob(os.path.join(ship_dir, '*.json')):
    base = os.path.basename(path)
    index = base.split('_', 1)[0]
    if not INDEX_RE.match(index):
        bad.append(('bad index', base))
        continue
    with open(path, encoding='utf-8', errors='replace') as f:
        txt = f.read()
    m = ID_RE.search(txt)
    if not m:
        bad.append(('no id', base))
        continue
    rows.append((index, int(m.group(1))))

rows.sort()
print(f'extracted: {len(rows)}')
print(f'skipped:   {len(bad)}')
for reason, name in bad[:10]:
    print('  skip', reason, name)

dup = len(rows) - len({i for i, _ in rows})
print(f'duplicate indexes: {dup}')

for r in rows[:5]:
    print('  sample:', r)

if write_to:
    with open(write_to, 'w', encoding='utf-8', newline='\n') as f:
        f.write('# index<TAB>ship_id  —— 给 tempArenaInfo.json 里的 playerVehicle 用\n')
        f.write('# 来源：Korabli 26.11 客户端 res_unpack/content/split/26.11/Ship/*.json 的顶层 id\n')
        f.write('# 用法：playerVehicle 形如 PASC710-Salem，取第一个 - 或 _ 之前的部分当 key\n')
        for index, sid in rows:
            f.write(f'{index}\t{sid}\n')
    print('written:', write_to)
