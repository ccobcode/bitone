// 两端共用的令牌,来源是 design/tokens.json.
import design from "../../../../design/tokens.json";

export const tokens = {
    qr: design.qr,
    /** 二维码以外的页面元素占用的高度. */
    qrChrome: design.size["h-row"] * 3 + design.space.s8,
    toast: design.motion.duration.toast,
};
