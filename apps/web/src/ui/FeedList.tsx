import { Link } from "react-router";
import type { FeedRow } from "../gateway/history";
import { Icon } from "./Icon";
import { minuteLabel, timeLabel } from "./labels";
import { Chip, Party } from "./primitives";
import { TokenIcon, networkIcon } from "./TokenIcon";
import { t, translate } from "../core/i18n";

const NOTE_TONES = { warn: "text-warn", danger: "text-danger", muted: "text-text-3" };
const STATUS_TONES = {
    neutral: "text-text-2",
    accent: "text-accent-text",
    ok: "text-ok",
    warn: "text-warn",
    danger: "text-danger",
};

function FeedIcon({ row, size }: { row: FeedRow; size: number }) {
    const { entry, network } = row;
    return <TokenIcon asset={entry} chain={entry && !entry.native ? networkIcon(network) : undefined} size={size} />;
}

/** 一笔交易。整行进详情页。已确认是常态，不显示标签，异常才显示。compact 用在没有按天分组的地方，时间带日期。 */
export function FeedItem({ row, compact }: { row: FeedRow; compact?: boolean }) {
    const time = row.time === null ? t("等待打包") : compact ? timeLabel(row.time) : minuteLabel(row.time);
    const flagged = row.tone !== "ok";
    return (
        <li className="row relative flex min-h-row items-center gap-s3 py-s2">
            <Link to={row.href} aria-label={`${row.title} ${row.amount}`} className="absolute inset-0 rounded-r2" />
            <FeedIcon row={row} size={32} />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-t3 font-medium">{row.title}</span>
                <span className="flex min-w-0 gap-s2 text-t1 text-text-3">
                    <span className="truncate">
                        <Party text={row.party} />
                    </span>
                    {row.note ? (
                        <span className={`truncate ${NOTE_TONES[row.noteTone]}`}>{translate(row.note)}</span>
                    ) : null}
                </span>
                {row.remark ? <span className="truncate text-t1 text-text-2">{row.remark}</span> : null}
            </span>
            {flagged && !compact ? (
                <span className="hidden shrink-0 @split:block">
                    <Chip tone={row.tone}>{row.status}</Chip>
                </span>
            ) : null}
            <span className="flex min-w-0 shrink-0 flex-col items-end text-right">
                <span className={`text-t3 ${row.direction === "in" ? "text-ok" : ""}`}>{row.amount}</span>
                {/* 放不下标签时，状态写在时间的位置。 */}
                {flagged ? (
                    <span className={`text-t1 ${STATUS_TONES[row.tone]} ${compact ? "" : "@split:hidden"}`}>
                        {row.status}
                    </span>
                ) : null}
                {flagged && compact ? null : (
                    <span className={`text-t1 text-text-3 ${flagged ? "hidden @split:block" : ""}`}>{time}</span>
                )}
            </span>
            <Icon name="caret" size={16} className="shrink-0 text-text-3" />
        </li>
    );
}
