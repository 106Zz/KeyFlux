# KeyFlux 图标

- `KeyFlux.png`：App 图标及项目首页标志。
- `KeyFluxDMG.png`：DMG 文件、挂载卷及首页下载入口图标。

两张源图均通过 OpenAI 内置 imagegen 工具生成，外部背景使用透明 alpha。构图采用深蓝色底、薄荷色 Shift 符号；DMG 版本延续相同标志并采用磁盘外形。图标按本仓库 MIT 许可分发。

打包脚本用 `sips` 和 `iconutil` 从源图生成标准 macOS `.icns` 尺寸。彩色图标只用于 App、安装包和文档；菜单栏仍显示原来的轻量文字 ⇧。

## App 生成提示词

```text
Use case: logo-brand. Create one final production macOS application icon for KeyFlux, a minimal native keyboard input switching utility. Square 1024 x 1024 asset. A single generous rounded-square keycap tile in deep ink navy with a subtle blue-green gradient and precise beveled rim, soft restrained dimensional lighting. Centered on it is one large clean geometric Shift key symbol (an upward arrow with open hollow outline, the familiar ⇧ keyboard glyph), in luminous pale mint. The arrow should be unmistakable and legible at 32 pixels. Beautiful restrained macOS desktop app aesthetic, simple geometric silhouette, exact symmetry, premium materials without excessive gloss. Icon occupies about 86 percent of the canvas, with completely transparent background outside the rounded tile, alpha transparency, no backdrop, no checkerboard pixels. No text, no app name, no extra arrows, no keyboard illustration, no border around entire image, no device mockup, no watermark. Deliver just the isolated app icon.
```

## DMG 生成提示词

以 `KeyFlux.png` 为参考图：

```text
Use case: logo-brand. Create the matching macOS DMG installer disk icon for the KeyFlux app in the supplied reference. Preserve the reference's deep navy, teal-blue materials and its exact pale mint outlined Shift symbol as the brand mark. A single beautifully crafted compact external disk drive viewed in a restrained slightly elevated front perspective, square 1024 x 1024 asset, isolated on genuinely transparent alpha background. Broad smooth dark blue top surface with the reference Shift mark prominently centered on the top; understated silver-blue lower front bevel, one tiny mint status light. Clean, recognizable disk-drive silhouette, visually coherent with the supplied app icon, excellent small-size legibility. No text, no letters DMG, no surrounding mockup, no drop shadow extending far outside icon, no extra symbols, no arrows other than the existing Shift mark. Output only one isolated installer disk icon.
```
