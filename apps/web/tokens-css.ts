// 把 design/tokens.json 变成 CSS 变量.用 PostCSS 节点构建,通过虚拟模块引入,不落盘.
import postcss from "postcss";
import type { Plugin } from "vite";
import design from "../../design/tokens.json";

type Declaration = [prop: string, value: string];
type Mode = "light" | "dark";

const { color, elevation, font, fontSize, lineHeight, motion, radius, size, space, web } = design;

const colors = (mode: Mode): Declaration[] =>
    Object.entries({ ...color, ...elevation, ...web.chart }).map(([name, value]) => [`--${name}`, value[mode]]);

const lengths = (group: Record<string, number>, prefix = ""): Declaration[] =>
    Object.entries(group).map(([name, value]) => [`--${prefix}${name}`, `${value}px`]);

const timing: Declaration[] = [
    ...Object.entries(motion.duration).map(([name, value]): Declaration => [`--dur-${name}`, `${value}ms`]),
    ...Object.entries(motion.easing).map(([name, value]): Declaration => [
        `--ease-${name}`,
        `cubic-bezier(${value.join(", ")})`,
    ]),
    ["--press-scale", String(motion["press-scale"])],
    ["--pop-scale", String(motion["pop-scale"])],
    ["--scrim", `${motion.scrim * 100}%`],
    ["--page-in", `${motion["page-in"]}px`],
    ["--page-out", `${motion["page-out"]}px`],
    ["--rise", `${motion.rise}px`],
];

function rule(selector: string, declarations: Declaration[]) {
    return postcss.rule({ selector }).append(...declarations.map(([prop, value]) => postcss.decl({ prop, value })));
}

function stylesheet(): string {
    const dark: Declaration[] = [...colors("dark"), ["color-scheme", "dark"]];
    const root = postcss.root();
    root.append(
        rule(":root", [
            ...colors("light"),
            ...lengths(space),
            ...lengths(radius),
            ...lengths(fontSize),
            ...lengths(lineHeight, "lh-"),
            ...lengths({ ...size, ...web.size }),
            ...timing,
            ["--font", font.sans],
            ["--mono", font.mono],
            ["color-scheme", "light"],
        ]),
        postcss
            .atRule({ name: "media", params: "(prefers-color-scheme: dark)" })
            .append(rule(":root:not([data-theme='light'])", dark)),
        rule(":root[data-theme='dark']", dark),
    );
    return root.toString();
}

const MODULE = "virtual:tokens.css";
const RESOLVED = `\0${MODULE}`;

export function tokensCss(): Plugin {
    return {
        name: "bitone-tokens-css",
        resolveId: (id) => (id === MODULE ? RESOLVED : undefined),
        load: (id) => (id === RESOLVED ? stylesheet() : undefined),
    };
}
