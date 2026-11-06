import type { CameraFailure } from "./Camera";
import { t } from "../core/i18n";

/** 摄像头异常的文案,W2 和 W4 共用. */
export const CAMERA_FAILURES: Record<
    CameraFailure,
    { title: string; body: string; facts: { label: string; value: string }[]; retry: string }
> = {
    denied: {
        title: t("摄像头被拒"),
        body: t("浏览器没有摄像头权限,可以改用图片上传."),
        facts: [
            { label: t("权限"), value: t("已拒绝") },
            { label: t("设备"), value: t("未读取") },
        ],
        retry: t("重新授权"),
    },
    missing: {
        title: t("没有摄像头"),
        body: t("这台设备读取不到摄像头,可以改用图片上传."),
        facts: [
            { label: t("权限"), value: t("未申请") },
            { label: t("设备"), value: t("未找到") },
        ],
        retry: t("重新检测"),
    },
    failed: {
        title: t("摄像头不可用"),
        body: t("摄像头打开失败,可能被其他应用占用,可以改用图片上传."),
        facts: [
            { label: t("权限"), value: t("未知") },
            { label: t("设备"), value: t("打开失败") },
        ],
        retry: t("重新打开"),
    },
};
