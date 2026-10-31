import { useEffect, useState } from "react";
import { POLL_INTERVAL, useGateway } from "../gateway/provider";
import { clockLabel } from "./labels";
import { StateCard } from "./StateCard";
import { Button, LinkButton } from "./primitives";
import { t } from "../core/i18n";

/** 网关断开,保留最后一次查询到的余额,未完成的交易改用轮询. */
export function GatewayDown() {
    const { lastUpdated, retryAt, reconnect } = useGateway();
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        const timer = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(timer);
    }, []);
    const seconds = retryAt ? Math.max(Math.ceil((retryAt - now) / 1000), 0) : 0;
    return (
        <StateCard
            tone="danger"
            icon="offline"
            title={t("连接已断开")}
            body={t("余额和交易状态可能不是最新.")}
            facts={[
                { label: t("最后更新"), value: lastUpdated ? clockLabel(lastUpdated) : t("暂无") },
                { label: t("自动重连"), value: retryAt ? t("{0} 秒后", seconds) : t("连接中") },
                { label: t("轮询间隔"), value: t("{0} 秒", POLL_INTERVAL / 1000) },
            ]}
            actions={
                <>
                    <LinkButton to="/settings">{t("查看设置")}</LinkButton>
                    <Button variant="primary" onClick={reconnect}>
                        {t("立即重连")}
                    </Button>
                </>
            }
        />
    );
}
