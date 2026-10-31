// 从 tokens.json 生成两端的令牌文件。加 --check 时只比对，不写入。
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const tokens = JSON.parse(readFileSync(join(root, "design/tokens.json"), "utf8"));
const header = "由 design/generate.mjs 生成，不要手改。";

/** `web` 是网页端专用的圆角和尺寸，只进 CSS，不影响冷钱包的 Kotlin 令牌。 */
function css() {
    const web = tokens.web;
    const colors = (mode) =>
        Object.entries({ ...tokens.color, ...tokens.elevation })
            .map(([k, v]) => `  --${k}: ${v[mode]};`)
            .join("\n");
    const px = (group) =>
        Object.entries(group)
            .map(([k, v]) => `  --${k}: ${v}px;`)
            .join("\n");
    const { duration, easing } = tokens.motion;
    const motion = [
        ...Object.entries(duration).map(([k, v]) => `  --dur-${k}: ${v}ms;`),
        ...Object.entries(easing).map(([k, v]) => `  --ease-${k}: cubic-bezier(${v.join(", ")});`),
        `  --press-scale: ${tokens.motion["press-scale"]};`,
    ].join("\n");
    return `/* ${header} */
:root {
${colors("light")}
${px(tokens.space)}
${px({ ...tokens.radius, ...web.radius })}
${px(tokens.fontSize)}
${px({ ...tokens.size, ...web.size })}
${motion}
  --font: ${tokens.font.sans};
  --mono: ${tokens.font.mono};
  color-scheme: light;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme='light']) {
${colors("dark").replaceAll("  --", "    --")}
    color-scheme: dark;
  }
}
:root[data-theme='dark'] {
${colors("dark")}
  color-scheme: dark;
}
`;
}

function camel(name) {
    return name.replace(/-(\w)/g, (_, c) => c.toUpperCase());
}

/** CSS 的 box-shadow 拆成层，Kotlin 用 Modifier.dropShadow 逐层画。 */
function shadowLayers(value) {
    if (value === "none") return [];
    return value.split(/,\s*(?=0 \d+px)/).map((layer) => {
        const [, y, blur, r, g, b, alpha] = layer.match(/^0 (\d+)px (\d+)px rgba\((\d+), (\d+), (\d+), ([\d.]+)\)$/);
        const hex = [Math.round(alpha * 255), r, g, b].map((n) => Number(n).toString(16).padStart(2, "0"));
        return `ShadowLayer(${y}.dp, ${blur}.dp, Color(0x${hex.join("").toUpperCase()}))`;
    });
}

function kotlin() {
    const names = Object.keys(tokens.color);
    const elevation = (mode) =>
        Object.entries(tokens.elevation)
            .map(([k, v]) => `    ${camel(k)} = listOf(${shadowLayers(v[mode]).join(", ")}),`)
            .join("\n");
    const palette = (mode) =>
        names.map((k) => `    ${camel(k)} = Color(0xFF${tokens.color[k][mode].slice(1)}),`).join("\n");
    const dp = (group, unit) =>
        Object.entries(group)
            .map(([k, v]) => `    val ${camel(k)} = ${v}.${unit}`)
            .join("\n");
    return `// ${header}
package com.bitone.wallet.ui.theme

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

data class ColorTokens(
${names.map((k) => `    val ${camel(k)}: Color,`).join("\n")}
)

val LightColors = ColorTokens(
${palette("light")}
)

val DarkColors = ColorTokens(
${palette("dark")}
)

/** 一层阴影，对应 CSS box-shadow 的纵向偏移、模糊半径和颜色。 */
data class ShadowLayer(val y: Dp, val blur: Dp, val color: Color)

data class ElevationTokens(
${Object.keys(tokens.elevation)
    .map((k) => `    val ${camel(k)}: List<ShadowLayer>,`)
    .join("\n")}
)

val LightElevation = ElevationTokens(
${elevation("light")}
)

val DarkElevation = ElevationTokens(
${elevation("dark")}
)

object Space {
${dp(tokens.space, "dp")}
}

object Radius {
${dp(tokens.radius, "dp")}
}

object FontSize {
${dp(tokens.fontSize, "sp")}
}

object Size {
${dp(tokens.size, "dp")}
}

/** 动效令牌。时长单位是毫秒。 */
object Motion {
${Object.entries(tokens.motion.duration)
    .map(([k, v]) => `    const val ${k} = ${v}`)
    .join("\n")}
${Object.entries(tokens.motion.easing)
    .map(([k, v]) => `    val ${k}Easing = CubicBezierEasing(${v.map((n) => n + "f").join(", ")})`)
    .join("\n")}
    const val pressScale = ${tokens.motion["press-scale"]}f
}
`;
}

/** 词典以中文原文为键，网页端和冷钱包共用。放不进 @Composable 以外的常量，所以生成一个普通的 Map。 */
const english = JSON.parse(readFileSync(join(root, "design/i18n.json"), "utf8"));

function kotlinString(text) {
    return JSON.stringify(text).replaceAll("$", "\\$");
}

function translations() {
    const rows = Object.entries(english).map(([zh, en]) => `    ${kotlinString(zh)} to ${kotlinString(en)},`);
    return `// ${header}
package com.bitone.wallet.i18n

internal val EN: Map<String, String> = mapOf(
${rows.join("\n")}
)
`;
}

const outputs = [
    ["apps/web/src/core/i18n.gen.json", `${JSON.stringify(english, null, 4)}\n`],
    ["apps/mobile/shared/src/commonMain/kotlin/com/bitone/wallet/i18n/Translations.kt", translations()],
    ["apps/web/src/styles/tokens.css", css()],
    ["apps/mobile/shared/src/commonMain/kotlin/com/bitone/wallet/ui/theme/Tokens.kt", kotlin()],
];

let stale = false;
for (const [path, content] of outputs) {
    const full = join(root, path);
    if (process.argv.includes("--check")) {
        let current = "";
        try {
            current = readFileSync(full, "utf8");
        } catch {}
        if (current !== content) {
            console.error(`过期：${path}`);
            stale = true;
        }
    } else {
        mkdirSync(dirname(full), { recursive: true });
        writeFileSync(full, content);
    }
}
process.exit(stale ? 1 : 0);
