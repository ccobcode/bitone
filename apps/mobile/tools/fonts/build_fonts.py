#!/usr/bin/env python3
"""生成 androidApp/src/main/res/font 下的字体文件.

字体打包进应用.Compose 的 FontFamily 不能按字符回退到第二个字体,
所以把 Inter 的拉丁字符和 Noto Sans SC 的中文合并成一个文件.

中文收录 GB2312 一级字和源码里用到的全部非 ASCII 字符,改文案一般不用重新生成.
数字直接写成 Inter 的等宽数字,合并后不带 GSUB,运行时的 tnum 特性不起作用.
行高度量统一,拉丁字母和中文在按钮,列表行里都垂直居中.
许可证随 APK 分发,放在 androidApp/src/main/assets/licenses.

依赖:pip install fonttools brotli
输入目录要有 google/fonts 仓库 ofl/ 下的这些文件:
  Inter[opsz,wght].ttf  NotoSansSC[wght].ttf  JetBrainsMono[wght].ttf
用法:python3 build_fonts.py <输入目录>
"""
import sys
from pathlib import Path

from fontTools import subset
from fontTools.merge import Merger, Options
from fontTools.ttLib import TTFont
from fontTools.ttLib.scaleUpem import scale_upem
from fontTools.varLib import instancer

MOBILE = Path(__file__).resolve().parents[2]
OUT = MOBILE / "androidApp/src/main/res/font"
WEIGHTS = {"regular": 400, "medium": 500, "semibold": 600}
ASCII = {chr(c) for c in range(0x20, 0x7F)}
DIGITS = "0123456789"
# Noto Sans SC 的单位.中文的字面中心约 0.38,Inter 大写字母的中心约 0.36,这组度量让两者都居中.
UPEM = 1000
ASCENT, DESCENT = 1000, -250


def gb2312_level1() -> set[str]:
    chars = set()
    for row in range(16, 56):
        for cell in range(1, 95):
            try:
                chars.add(bytes([row + 0xA0, cell + 0xA0]).decode("gb2312"))
            except UnicodeDecodeError:
                pass
    return chars


def source_chars() -> set[str]:
    chars = set()
    for path in (MOBILE / "shared/src/commonMain").rglob("*.kt"):
        chars.update(c for c in path.read_text("utf-8") if ord(c) > 0x7F)
    return chars


def subsetted(font: TTFont, chars: set[str], features: tuple[str, ...] = ()) -> TTFont:
    options = subset.Options()
    options.layout_features = list(features)
    options.name_IDs = ["*"]
    options.notdef_outline = True
    options.hinting = False
    subsetter = subset.Subsetter(options)
    subsetter.populate(text="".join(sorted(chars)))
    subsetter.subset(font)
    return font


def tabular_digits(font: TTFont) -> None:
    """把 tnum 里数字的替换写进 cmap.tnum 还会改句号和逗号,那些保持比例宽度."""
    gsub = font["GSUB"].table
    lookups = {i for r in gsub.FeatureList.FeatureRecord if r.FeatureTag == "tnum" for i in r.Feature.LookupListIndex}
    tabular = {}
    for index in lookups:
        for table in gsub.LookupList.Lookup[index].SubTable:
            tabular.update(getattr(table, "ExtSubTable", table).mapping)
    best = font.getBestCmap()
    digits = {digit: tabular[best[digit]] for digit in map(ord, DIGITS)}
    for table in font["cmap"].tables:
        if table.isUnicode():
            table.cmap.update(digits)


def even_metrics(font: TTFont) -> None:
    font["hhea"].ascent, font["hhea"].descent, font["hhea"].lineGap = ASCENT, DESCENT, 0
    os2 = font["OS/2"]
    os2.sTypoAscender, os2.sTypoDescender, os2.sTypoLineGap = ASCENT, DESCENT, 0
    os2.usWinAscent, os2.usWinDescent = ASCENT, -DESCENT
    os2.fsSelection |= 1 << 7


def rename(font: TTFont, family: str, style: str) -> None:
    for record in list(font["name"].names):
        if record.nameID in (1, 4, 16):
            font["name"].setName(family, record.nameID, record.platformID, record.platEncID, record.langID)
        elif record.nameID == 2:
            font["name"].setName(style, 2, record.platformID, record.platEncID, record.langID)
        elif record.nameID == 6:
            font["name"].setName(f"{family.replace(' ', '')}-{style}", 6, record.platformID, record.platEncID, record.langID)
        elif record.nameID in (3, 17, 21, 22, 25):
            font["name"].removeNames(nameID=record.nameID)


def build_sans(src: Path) -> None:
    cjk = (gb2312_level1() | source_chars()) - ASCII
    for name, weight in WEIGHTS.items():
        inter = instancer.instantiateVariableFont(TTFont(src / "Inter[opsz,wght].ttf"), {"wght": weight, "opsz": 14})
        noto = instancer.instantiateVariableFont(TTFont(src / "NotoSansSC[wght].ttf"), {"wght": weight})
        tabular_digits(inter)
        scale_upem(inter, UPEM)
        latin = subsetted(inter, ASCII | set("\u201c\u201d\u2018\u2019…—–"), ("kern",))
        hans = subsetted(noto, cjk)
        latin_path, hans_path = OUT / f".latin_{name}.ttf", OUT / f".hans_{name}.ttf"
        latin.save(latin_path)
        hans.save(hans_path)
        merged = Merger(Options(drop_tables=["vmtx", "vhea", "VORG", "DSIG", "meta", "STAT", "GSUB"])).merge([str(latin_path), str(hans_path)])
        rename(merged, "BitOne Sans", name.capitalize())
        merged["OS/2"].usWeightClass = weight
        even_metrics(merged)
        merged.save(OUT / f"bitone_sans_{name}.ttf")
        latin_path.unlink()
        hans_path.unlink()


def build_mono(src: Path) -> None:
    for name, weight in WEIGHTS.items():
        font = instancer.instantiateVariableFont(TTFont(src / "JetBrainsMono[wght].ttf"), {"wght": weight})
        font = subsetted(font, ASCII | set("…—"))
        rename(font, "BitOne Mono", name.capitalize())
        font["OS/2"].usWeightClass = weight
        font.save(OUT / f"bitone_mono_{name}.ttf")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    source = Path(sys.argv[1])
    build_sans(source)
    build_mono(source)
