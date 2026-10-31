import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// 与 docs/04-security.md 第 3 节一致.生产托管要下发同样的响应头.
const csp = [
    "default-src 'none'",
    "script-src 'self' 'wasm-unsafe-eval'",
    "style-src 'self'",
    "font-src 'self'",
    "img-src 'self'",
    "manifest-src 'self'",
    "connect-src 'self'",
    "base-uri 'none'",
    "form-action 'none'",
    "frame-ancestors 'none'",
].join("; ");

const proxy = { "/v1": { target: "http://127.0.0.1:8787", ws: true } };

export default defineConfig({
    plugins: [react(), tailwindcss()],
    build: {
        // 小文件内联成 data: 会被 CSP 拦下,所以字体,图标一律输出成同源文件.
        assetsInlineLimit: 0,
    },
    server: {
        proxy,
        // 图标从 design/icons 读取.
        fs: { allow: ["../.."] },
    },
    preview: {
        proxy,
        headers: { "Content-Security-Policy": csp },
    },
    test: {
        environment: "node",
        include: ["src/**/*.test.ts"],
        setupFiles: ["src/test-setup.ts"],
    },
});
