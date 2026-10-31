import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { FrameSender } from "../core";
import { tokens } from "../core/tokens";
import { Icon } from "./Icon";
import { Button } from "./primitives";
import { QrCode } from "./QrCode";
import { t } from "../core/i18n";

const SPEEDS = [
    { label: t("标准"), interval: tokens.qr.interval.normal },
    { label: t("慢"), interval: tokens.qr.interval.slow },
    { label: t("快"), interval: tokens.qr.interval.fast },
];

interface AnimatedQrProps {
    message: Uint8Array;
    type: "request" | "manifest";
    size: number;
    label: string;
    onError?(message: string): void;
}

/** 动态二维码,循环播放,可暂停,调速,全屏.喷泉码后面的帧也能用,所以一直发新帧. */
export function AnimatedQr({ message, type, size, label, onError }: AnimatedQrProps) {
    const sender = useRef<FrameSender | null>(null);
    const box = useRef<HTMLDivElement>(null);
    const [part, setPart] = useState<{ text: string; index: number; total: number } | null>(null);
    const [paused, setPaused] = useState(false);
    const [speed, setSpeed] = useState(0);
    const [fullscreen, setFullscreen] = useState(false);
    // 回调只用于报告,换一个回调不用重新分帧.
    const report = useRef(onError);
    useEffect(() => {
        report.current = onError;
    });

    // 第一帧要和外框一起上屏,否则下面的按钮会先出现再被推开.
    useLayoutEffect(() => {
        let current: FrameSender;
        try {
            current = new FrameSender(message, type);
        } catch (error) {
            report.current?.(error instanceof Error ? error.message : t("二维码生成失败"));
            return;
        }
        sender.current = current;
        setPart({ text: current.next(), index: 0, total: current.count });
        return () => {
            sender.current = null;
            current.free();
        };
    }, [message, type]);

    const interval = SPEEDS[speed]?.interval ?? tokens.qr.interval.normal;
    const single = part?.total === 1;
    useEffect(() => {
        if (paused || single) return;
        const timer = setInterval(() => {
            const current = sender.current;
            if (!current) return;
            const text = current.next();
            setPart((last) => ({ text, index: (last?.index ?? 0) + 1, total: current.count }));
        }, interval);
        return () => clearInterval(timer);
    }, [paused, interval, single]);

    useEffect(() => {
        const sync = () => setFullscreen(document.fullscreenElement === box.current);
        document.addEventListener("fullscreenchange", sync);
        return () => document.removeEventListener("fullscreenchange", sync);
    }, []);

    function toggleFullscreen() {
        if (document.fullscreenElement) void document.exitFullscreen();
        else void box.current?.requestFullscreen();
    }

    const shown = fullscreen ? Math.min(window.innerHeight - 140, window.innerWidth - 48) : size;

    return (
        <div
            ref={box}
            className={`flex flex-col gap-s4 ${fullscreen ? "h-full items-center justify-center bg-surface p-s6" : ""}`}
        >
            {part ? (
                <div className="flex w-full justify-center">
                    <QrCode
                        value={part.text}
                        size={shown}
                        label={label}
                        frame={(part.index % part.total) + 1}
                        total={part.total}
                    />
                </div>
            ) : null}
            <div className="flex flex-wrap justify-center gap-s2">
                <Button size="sm" disabled={single} onClick={() => setPaused(!paused)}>
                    {paused ? t("播放") : t("暂停")}
                </Button>
                <Button size="sm" disabled={single} onClick={() => setSpeed((speed + 1) % SPEEDS.length)}>
                    {t("速度 {0}", SPEEDS[speed]?.label ?? "")}
                </Button>
                {/* iPhone 没有全屏接口. */}
                {document.fullscreenEnabled ? (
                    <Button
                        size="iconSm"
                        aria-label={fullscreen ? t("退出全屏") : t("全屏显示二维码")}
                        onClick={toggleFullscreen}
                    >
                        <Icon name="expand" size={16} />
                    </Button>
                ) : null}
            </div>
        </div>
    );
}
