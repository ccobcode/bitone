import { Menu } from "@base-ui/react/menu";
import { useState, type ReactNode } from "react";
import { Icon } from "./Icon";
import { buttonClass } from "./primitives";
import { t } from "../core/i18n";

export interface PickerItem {
    key: string;
    node: ReactNode;
    disabled?: boolean;
    selected?: boolean;
}

interface PickerProps {
    id: string;
    value: ReactNode;
    items: PickerItem[];
    onPick(key: string): void;
    /** field 是通栏，compact 和输入框等高，filter 和分段选择等高，pill 是金额旁边的胶囊。 */
    look?: "field" | "compact" | "filter" | "pill";
    label?: string;
    empty?: string;
}

const LOOKS = {
    field: "h-14 w-full rounded-r2 px-s4",
    compact: "h-input shrink-0 rounded-r2 px-s4 text-t2 font-medium text-text-2",
    filter: "h-11 shrink-0 rounded-r2 px-s4 text-t2 font-medium text-text-2 nav:h-btn",
    pill: "h-12 shrink-0 rounded-rf py-s1 pr-s3 pl-s2",
};

/** 下拉选择，内容带图标。宽屏贴着触发它的元素展开，手机上是底部弹层，有标题和关闭按钮。 */
export function Picker({ id, value, items, onPick, look = "field", label, empty = t("没有可选项") }: PickerProps) {
    const [open, setOpen] = useState(false);
    return (
        <Menu.Root open={open} onOpenChange={setOpen}>
            <Menu.Trigger
                id={id}
                aria-label={label}
                className={`press flex min-w-0 items-center gap-s3 bg-fill text-left hover:bg-fill-hover data-[popup-open]:bg-fill-hover ${LOOKS[look]}`}
            >
                <span className="flex min-w-0 flex-1 items-center gap-s3">{value}</span>
                <Icon name="chevron" size={16} className="flip shrink-0 text-text-3" />
            </Menu.Trigger>
            <Menu.Portal>
                <Menu.Backdrop className="scrim fixed inset-0 z-30 nav:hidden" />
                <Menu.Positioner
                    sideOffset={8}
                    align={look === "field" ? "start" : "end"}
                    className="picker-position z-40"
                >
                    <Menu.Popup className="popup picker max-h-96 overflow-y-auto nav:min-w-[max(var(--anchor-width),16rem)]">
                        {label ? (
                            <div className="picker-title">
                                <span className="min-w-0 flex-1 truncate">{label}</span>
                                <button
                                    type="button"
                                    aria-label={t("关闭")}
                                    tabIndex={-1}
                                    onClick={() => setOpen(false)}
                                    className={buttonClass("ghost", "iconSm")}
                                >
                                    <Icon name="close" />
                                </button>
                            </div>
                        ) : null}
                        {items.length === 0 ? (
                            <div className="px-s3 py-s4 text-center text-t3 text-text-2">{empty}</div>
                        ) : null}
                        {items.map((item) => (
                            <Menu.Item
                                key={item.key}
                                disabled={item.disabled}
                                onClick={() => onPick(item.key)}
                                data-selected={item.selected ? "" : undefined}
                                className="popup-item cursor-pointer"
                            >
                                {item.node}
                            </Menu.Item>
                        ))}
                    </Menu.Popup>
                </Menu.Positioner>
            </Menu.Portal>
        </Menu.Root>
    );
}
