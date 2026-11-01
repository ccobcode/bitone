const ZOOM_KEYS = new Set(["+", "-", "=", "0"]);

const cancel = (event: Event) => event.preventDefault();

/** iOS Safari 不理会 viewport 的 `user-scalable=no`,桌面浏览器也有自己的缩放快捷方式,都在这里拦下. */
export function blockZoom(target: Window) {
    const active = { passive: false };
    for (const gesture of ["gesturestart", "gesturechange", "gestureend"])
        target.addEventListener(gesture, cancel, active);
    target.addEventListener(
        "touchmove",
        (event) => {
            if ((event as TouchEvent).touches.length > 1) event.preventDefault();
        },
        active,
    );
    target.addEventListener(
        "wheel",
        (event) => {
            if ((event as WheelEvent).ctrlKey) event.preventDefault();
        },
        active,
    );
    target.addEventListener("keydown", (event) => {
        const { ctrlKey, metaKey, key } = event as KeyboardEvent;
        if ((ctrlKey || metaKey) && ZOOM_KEYS.has(key)) event.preventDefault();
    });
}
