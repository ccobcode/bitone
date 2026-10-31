import { CSPProvider } from "@base-ui/react/csp-provider";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { initCore } from "./core";
import { AppProvider } from "./data/app";
import { PageRouter } from "./ui/PageRouter";
import "virtual:tokens.css";
import "./styles/index.css";
import { t } from "./core/i18n";

const client = new QueryClient({
    defaultOptions: {
        queries: {
            // 默认在浏览器报告离线时暂停,页面会一直停在加载中,看不到网关断开.
            networkMode: "always",
            staleTime: 30_000,
        },
    },
});

async function boot(root: HTMLElement) {
    await initCore();
    createRoot(root).render(
        <StrictMode>
            {/* Base UI 默认会插入内联 style 标签,CSP 不允许. */}
            <CSPProvider disableStyleElements>
                <QueryClientProvider client={client}>
                    <PageRouter>
                        <AppProvider>
                            <App />
                        </AppProvider>
                    </PageRouter>
                </QueryClientProvider>
            </CSPProvider>
        </StrictMode>,
    );
}

const root = document.getElementById("root");
if (root) {
    boot(root).catch(() => {
        root.textContent = t("启动失败,请刷新页面.");
    });
}
