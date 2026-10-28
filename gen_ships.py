"""从 Lesta 汉化包的 global.mo 重新生成船名表。

用法：
    # 1. 从 .mkmod（本质是 zip）里取出 global.mo
    unzip -o "MK_L10N_CHS.mkmod" "texts/ru/LC_MESSAGES/global.mo" -d /tmp/mkmod

    # 2. 生成（不加 --write 只打印统计，不动文件）
    python gen_ships.py <global.mo 的 Windows 路径> chatrans/ship_index.tsv chatrans/ship_ids.tsv --write

产出：
    chatrans/ship_ids.tsv     id    -> 名字（只更新能通过 index 对上号的条目）
    chatrans/ship_names.tsv   index -> 名字（全量，含还没有数字 ID 的新船）

注意：这里的 python 是 Windows 原生程序，不认 /tmp 这种 MSYS 路径，
      用 cygpath -w 转成 Windows 路径再传进来。
"""

import struct
import sys
import re

SHIP_KEY = re.compile(r'^IDS_(P[A-Z]{2}[A-Z]\d{3})_FULL$')
SUFFIX = re.compile(r'\s*\(<[^)]*\)\s*$')
HEADER_LINES = {
    'ship_ids.tsv': [
        '# id<TAB>ship display name',
        '# 中文名来源：DDFantasyV/Korabli_localization_chs 的 global.mo (26.11, 2026-10-28)',
        '#   通过 ship_index.tsv 的 index 与 IDS_<index>_FULL 对应',
        '# 没有中文名时保留英文（GameParams 配置名后缀）',
    ],
    'ship_names.tsv': [
        '# index<TAB>ship display name',
        '# 中文名来源：DDFantasyV/Korabli_localization_chs 的 global.mo (26.11, 2026-10-28)',
        '# 键是 IDS_<index>_FULL 里的 index，用于 tempArenaInfo.json 的 playerVehicle，',
        '# 覆盖包括「没有数字 ID 的新船」在内的全部条目；查不到时回退到 ship_ids.tsv',
    ],
}


def parse_mo(path):
    d = open(path, 'rb').read()
    endian = '<'
    if struct.unpack('<I', d[0:4])[0] != 0x950412de:
        endian = '>'
        assert struct.unpack('>I', d[0:4])[0] == 0x950412de, 'not MO'
    n, oo, to = struct.unpack(endian + 'III', d[8:20])
    out = {}
    for i in range(n):
        ol, ooff = struct.unpack(endian + 'II', d[oo + i * 8: oo + i * 8 + 8])
        tl, toff = struct.unpack(endian + 'II', d[to + i * 8: to + i * 8 + 8])
        out[d[ooff:ooff + ol]] = d[toff:toff + tl]
    return out


mo = parse_mo(sys.argv[1])
index_path = sys.argv[2]
ids_path = sys.argv[3]
write = '--write' in sys.argv

# index -> id（保持文件原顺序）
index_rows = []
for line in open(index_path, encoding='utf-8'):
    line = line.rstrip('\n')
    if not line or line.startswith('#'):
        continue
    p = line.split('\t')
    if len(p) == 2:
        index_rows.append((p[0].strip(), p[1].strip()))

# id -> name（保持文件原顺序）
id_rows = []
for line in open(ids_path, encoding='utf-8'):
    line = line.rstrip('\n')
    if not line or line.startswith('#'):
        continue
    p = line.split('\t')
    if len(p) == 2:
        id_rows.append((p[0].strip(), p[1].strip()))

# MO 里的船名
ships = {}
for k, v in mo.items():
    try:
        s = k.decode('utf-8')
    except UnicodeDecodeError:
        continue
    m = SHIP_KEY.match(s)
    if not m:
        continue
    name = SUFFIX.sub('', v.decode('utf-8', 'replace')).strip()
    if name:
        ships[m.group(1)] = name

# 1) 更新 id -> name：凡是能通过 index 找到新名字的，一律采用
updated = 0
new_id_rows = []
for sid, old_name in id_rows:
    new_id_rows.append((sid, old_name))
index_to_id = {idx: sid for idx, sid in index_rows}
id_pos = {sid: i for i, (sid, _) in enumerate(new_id_rows)}
for idx, sid in index_rows:
    if idx in ships and sid in id_pos and new_id_rows[id_pos[sid]][1] != ships[idx]:
        new_id_rows[id_pos[sid]] = (sid, ships[idx])
        updated += 1

# 2) 生成 index -> name（全量，含没有数字 ID 的新船）
name_rows = sorted(ships.items())

print(f'id 表更新条目: {updated}')
print(f'index 名字表条目: {len(name_rows)}')
print(f'其中无数字 ID 的: {len([i for i, _ in name_rows if i not in index_to_id])}')

if write:
    def dump(path, header_key, rows):
        with open(path, 'w', encoding='utf-8', newline='\n') as f:
            f.write('\n'.join(HEADER_LINES[header_key]) + '\n')
            for a, b in rows:
                f.write(f'{a}\t{b}\n')

    dump(ids_path, 'ship_ids.tsv', new_id_rows)
    out_names = ids_path.replace('ship_ids.tsv', 'ship_names.tsv')
    dump(out_names, 'ship_names.tsv', name_rows)
    print('已写入:', ids_path, out_names)
else:
    print('dry-run，加 --write 才会写文件')
