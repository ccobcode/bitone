import { Menu } from "@base-ui/react/menu";
import type { ReactNode } from "react";
import { Icon } from "./Icon";
import { t } from "../core/i18n";

export interface PickerItem {
    key: string;
    node: ReactNode;
    disabled?: boolean;
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
    filter: "h-btn shrink-0 rounded-r2 px-s4 text-t2 font-medium text-text-2",
    pill: "h-12 shrink-0 rounded-rf py-s1 pr-s3 pl-s2",
};

/** 下拉选择，内容带图标。与原型的付款账户、资产选择一致。 */
export function Picker({ id, value, items, onPick, look = "field", label, empty = t("没有可选项") }: PickerProps) {
    return (
        <Menu.Root>
            <Menu.Trigger
                id={id}
                aria-label={label}
                className={`press flex min-w-0 items-center gap-s3 bg-fill text-left hover:bg-fill-hover data-[popup-open]:bg-fill-hover ${LOOKS[look]}`}
            >
                <span className="flex min-w-0 flex-1 items-center gap-s3">{value}</span>
                <Icon name="chevron" size={16} className="flip shrink-0 text-text-3" />
            </Menu.Trigger>
            <Menu.Portal>
                <Menu.Positioner sideOffset={8} align={look === "field" ? "start" : "end"} className="z-40">
                    <Menu.Popup className="popup max-h-96 min-w-[max(var(--anchor-width),16rem)] overflow-y-auto">
                        {items.length === 0 ? (
                            <div className="px-s3 py-s4 text-center text-t3 text-text-2">{empty}</div>
                        ) : null}
                        {items.map((item) => (
                            <Menu.Item
                                key={item.key}
                                disabled={item.disabled}
                                onClick={() => onPick(item.key)}
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
