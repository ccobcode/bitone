import { useEffect, useRef, useState } from "react";
import { t } from "../core/i18n";
import { CAMERA_FAILURES } from "./cameraFailures";
import { Button } from "./primitives";
import { StateCard } from "./StateCard";

export type CameraFailure = "denied" | "missing" | "failed";

/** 送去识别的图像最长边，够看清单帧二维码，也不拖慢每帧的解码。 */
const MAX_EDGE = 1280;

function failureOf(error: unknown): CameraFailure {
    const name = (error as { name?: string } | null)?.name;
    if (name === "NotAllowedError" || name === "SecurityError") return "denied";
    if (name === "NotFoundError" || name === "OverconstrainedError") return "missing";
    return "failed";
}

function stopTracks(stream: MediaStream): void {
    for (const track of stream.getTracks()) track.stop();
}

async function scanLoop(video: HTMLVideoElement, stopped: () => boolean, onText: (text: string) => void) {
    const { decodeQr } = await import("../scan/zxing");
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error(t("无法读取画面"));
    while (!stopped()) {
        if (video.videoWidth > 0) {
            const scale = Math.min(1, MAX_EDGE / Math.max(video.videoWidth, video.videoHeight));
            canvas.width = Math.round(video.videoWidth * scale);
            canvas.height = Math.round(video.videoHeight * scale);
            context.drawImage(video, 0, 0, canvas.width, canvas.height);
            const [text] = await decodeQr(context.getImageData(0, 0, canvas.width, canvas.height));
            if (text && !stopped()) onText(text);
        }
        await new Promise(requestAnimationFrame);
    }
}

interface CameraProps {
    active: boolean;
    deviceId: string;
    onText(text: string): void;
    onFailure(failure: CameraFailure | null): void;
    onDevices(devices: MediaDeviceInfo[]): void;
}

/** 收齐时短促震动，和冷钱包相同。只有部分手机浏览器支持，其余什么都不做。 */
export function buzz(): void {
    navigator.vibrate?.(30);
}

/** 实时取景，逐帧识别。取景区域在任何主题下都是深色。 */
export function Camera({ active, deviceId, onText, onFailure, onDevices }: CameraProps) {
    const video = useRef<HTMLVideoElement>(null);
    const callbacks = useRef({ onText, onFailure, onDevices });
    useEffect(() => {
        callbacks.current = { onText, onFailure, onDevices };
    });

    useEffect(() => {
        const element = video.current;
        if (!active || !element) return;
        let stopped = false;
        let stream: MediaStream | null = null;

        async function run(target: HTMLVideoElement) {
            if (!navigator.mediaDevices?.getUserMedia) throw Object.assign(new Error(), { name: "NotFoundError" });
            const constraints = deviceId
                ? { deviceId: { exact: deviceId } }
                : { facingMode: "environment", width: { ideal: 1280 }, height: { ideal: 720 } };
            const opened = await navigator.mediaDevices.getUserMedia({ video: constraints, audio: false });
            // 申请期间页面可能已经离开，这时要立刻关掉，否则摄像头一直亮着。
            if (stopped) {
                stopTracks(opened);
                return;
            }
            stream = opened;
            target.srcObject = opened;
            await target.play();
            callbacks.current.onFailure(null);
            callbacks.current.onDevices(
                (await navigator.mediaDevices.enumerateDevices()).filter((device) => device.kind === "videoinput"),
            );
            await scanLoop(
                target,
                () => stopped,
                (text) => callbacks.current.onText(text),
            );
        }

        run(element).catch((error: unknown) => {
            if (!stopped) callbacks.current.onFailure(failureOf(error));
        });
        return () => {
            stopped = true;
            if (stream) stopTracks(stream);
            element.srcObject = null;
        };
    }, [active, deviceId]);

    return (
        <div className="relative flex aspect-[4/3] items-end justify-center overflow-hidden rounded-r3 bg-camera">
            <video ref={video} muted playsInline className="absolute inset-0 size-full object-cover" />
            <span aria-hidden="true" className="scan-frame">
                <span className="scan-corner" />
                <span className="scan-corner" />
                <span className="scan-corner" />
                <span className="scan-corner" />
                {active ? <span className="scan-sweep" /> : null}
            </span>
            <span className="relative mb-s3 text-t2 text-on-camera">{active ? t("对准冷钱包屏幕") : t("已收齐")}</span>
        </div>
    );
}

interface CameraPanelProps {
    active: boolean;
    failure: CameraFailure | null;
    onFailure(failure: CameraFailure | null): void;
    onText(text: string): void;
    /** 摄像头打不开时的退路。 */
    onUpload(): void;
}

/** 取景区域加摄像头选择。打不开时换成异常卡片，可以重试或者上传图片。 */
export function CameraPanel({ active, failure, onFailure, onText, onUpload }: CameraPanelProps) {
    const [attempt, setAttempt] = useState(0);
    const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
    const [deviceId, setDeviceId] = useState("");

    if (failure) {
        const state = CAMERA_FAILURES[failure];
        return (
            <StateCard
                tone="warn"
                icon="cameraOff"
                title={state.title}
                body={state.body}
                facts={state.facts}
                actions={
                    <>
                        <Button onClick={onUpload}>{t("上传图片")}</Button>
                        <Button
                            variant="primary"
                            onClick={() => {
                                onFailure(null);
                                setAttempt(attempt + 1);
                            }}
                        >
                            {state.retry}
                        </Button>
                    </>
                }
            />
        );
    }
    return (
        <>
            {devices.length > 1 ? (
                <select
                    aria-label={t("摄像头")}
                    value={deviceId}
                    onChange={(event) => setDeviceId(event.target.value)}
                    className="field text-t3"
                >
                    <option value="">{t("默认摄像头")}</option>
                    {devices.map((device, i) => (
                        <option key={device.deviceId} value={device.deviceId}>
                            {device.label || t("摄像头 {0}", i + 1)}
                        </option>
                    ))}
                </select>
            ) : null}
            <Camera
                key={attempt}
                active={active}
                deviceId={deviceId}
                onText={onText}
                onFailure={onFailure}
                onDevices={setDevices}
            />
        </>
    );
}

/** 从图片读二维码。隐藏的文件框放进页面，`open` 弹出选择。 */
export function useImageScan(label: string, onText: (text: string) => void, onNotice: (text: string) => void) {
    const input = useRef<HTMLInputElement>(null);

    async function read(files: File[]) {
        try {
            const { decodeQr } = await import("../scan/zxing");
            for (const file of files) {
                const [text] = await decodeQr(file);
                if (text) onText(text);
                else onNotice(t("图片里没有识别到二维码。"));
            }
        } catch {
            onNotice(t("图片无法读取。"));
        }
    }

    return {
        open: () => input.current?.click(),
        element: (
            <input
                ref={input}
                type="file"
                accept="image/*"
                multiple
                hidden
                aria-label={label}
                onChange={(event) => {
                    // FileList 是实时的，清空 value 后就没有内容了，所以先拷出来。
                    void read([...(event.target.files ?? [])]);
                    event.target.value = "";
                }}
            />
        ),
    };
}
