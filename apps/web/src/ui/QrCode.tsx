import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { qrMatrix, qrText } from "../core";
import tokens from "../core/tokens.gen.json";

/** 四周的静区,单位是码元,见 docs/06-ui.md 第 4.5 节. */
const QUIET = tokens.qr.quiet;

/** 帧数太多时分段画不下,改成一条连续的进度条. */
const MAX_SEGMENTS = tokens.qr["max-segments"];

function modulesPath(size: number, modules: boolean[]): string {
    let path = "";
    for (let y = 0; y < size; y++) {
        let x = 0;
        while (x < size) {
            if (!modules[y * size + x]) {
                x += 1;
                continue;
            }
            const start = x;
            while (x < size && modules[y * size + x]) x += 1;
            path += `M${start} ${y}h${x - start}v1h${start - x}z`;
        }
    }
    return path;
}

interface QrCodeProps {
    value: string;
    /** 宽屏的外框边长.手机上铺满可用宽度,和冷钱包相同.码元取整数像素,放不满的部分是白边. */
    size: number;
    label: string;
    /** 内容是普通文本,不是 UR 帧. */
    plain?: boolean;
    frame?: number;
    total?: number;
}

/** 任何主题下都是白底深色码点. */
export function QrCode({ value, size, label, plain = false, frame = 1, total = 1 }: QrCodeProps) {
    const matrix = useMemo(() => (plain ? qrText(value) : qrMatrix(value)), [plain, value]);
    const path = useMemo(() => modulesPath(matrix.size, matrix.modules), [matrix]);
    const box = useRef<HTMLDivElement>(null);
    const [available, setAvailable] = useState(size);

    useLayoutEffect(() => {
        const element = box.current;
        if (!element) return;
        setAvailable(element.clientWidth);
        const observer = new ResizeObserver(() => setAvailable(element.clientWidth));
        observer.observe(element);
        return () => observer.disconnect();
    }, []);

    // 横屏时按屏幕高度收窄,一屏看得全.
    const side = Math.min(available, window.innerHeight - tokens.qrChrome);
    const module = Math.max(Math.floor(side / (matrix.size + QUIET * 2)), 1);
    const offset = Math.floor((side - matrix.size * module) / 2);

    return (
        <div ref={box} className="qr-box w-full" style={{ "--qr": `${size}px` } as CSSProperties}>
            <div className="flex flex-col gap-s3" style={{ width: side }}>
                <svg
                    width={side}
                    height={side}
                    viewBox={`0 0 ${side} ${side}`}
                    shapeRendering="crispEdges"
                    role="img"
                    aria-label={label}
                    className="block rounded-r3"
                >
                    <rect width={side} height={side} fill="#FFFFFF" />
                    <path d={path} fill="#12151A" transform={`translate(${offset} ${offset}) scale(${module})`} />
                </svg>
                {total > 1 ? (
                    <div className="flex items-center gap-s3">
                        {total > MAX_SEGMENTS ? (
                            <span
                                aria-hidden="true"
                                className="h-1 min-w-0 flex-1 overflow-hidden rounded-rf bg-line-strong"
                            >
                                <span
                                    className="block h-full bg-accent"
                                    style={{ width: `${(frame / total) * 100}%` }}
                                />
                            </span>
                        ) : (
                            <span aria-hidden="true" className="flex min-w-0 flex-1 gap-s1">
                                {Array.from({ length: total }, (_, i) => i + 1).map((segment) => (
                                    <span
                                        key={segment}
                                        className={`h-1 flex-1 rounded-rf ${segment === frame ? "bg-accent" : "bg-line-strong"}`}
                                    />
                                ))}
                            </span>
                        )}
                        <span className="text-t2 text-text-2">
                            {frame}/{total}
                        </span>
                    </div>
                ) : null}
            </div>
        </div>
    );
}
