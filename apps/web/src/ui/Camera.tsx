import { useEffect, useRef } from "react";
import { t } from "../core/i18n";

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
            <span
                aria-hidden="true"
                className="absolute top-[16%] left-[22%] size-7 rounded-tl-r1 border-t-[3px] border-l-[3px] border-on-camera"
            />
            <span
                aria-hidden="true"
                className="absolute top-[16%] right-[22%] size-7 rounded-tr-r1 border-t-[3px] border-r-[3px] border-on-camera"
            />
            <span
                aria-hidden="true"
                className="absolute bottom-[16%] left-[22%] size-7 rounded-bl-r1 border-b-[3px] border-l-[3px] border-on-camera"
            />
            <span
                aria-hidden="true"
                className="absolute right-[22%] bottom-[16%] size-7 rounded-br-r1 border-r-[3px] border-b-[3px] border-on-camera"
            />
            <span className="relative mb-s3 text-t2 text-on-camera">{active ? t("对准冷钱包屏幕") : t("已收齐")}</span>
        </div>
    );
}
