import { describe, expect, it } from "vitest";
import { blockZoom } from "./noZoom";

function fired(target: EventTarget, type: string, fields: Record<string, unknown> = {}) {
    const event = Object.assign(new Event(type, { cancelable: true }), fields);
    target.dispatchEvent(event);
    return event.defaultPrevented;
}

describe("blockZoom", () => {
    const target = new EventTarget() as Window;
    blockZoom(target);

    it("拦下 iOS 的捏合手势", () => {
        expect(fired(target, "gesturestart")).toBe(true);
        expect(fired(target, "touchmove", { touches: [{}, {}] })).toBe(true);
    });

    it("单指滚动不受影响", () => {
        expect(fired(target, "touchmove", { touches: [{}] })).toBe(false);
    });

    it("拦下桌面的缩放快捷方式", () => {
        expect(fired(target, "wheel", { ctrlKey: true })).toBe(true);
        expect(fired(target, "keydown", { ctrlKey: true, key: "=" })).toBe(true);
        expect(fired(target, "keydown", { metaKey: true, key: "-" })).toBe(true);
    });

    it("普通滚轮和按键照常", () => {
        expect(fired(target, "wheel")).toBe(false);
        expect(fired(target, "keydown", { key: "=" })).toBe(false);
        expect(fired(target, "keydown", { ctrlKey: true, key: "c" })).toBe(false);
    });
});
