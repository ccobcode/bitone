import type { FeedRow } from "../gateway/history";
import { minuteLabel, timeLabel } from "./labels";
import { Chip, ListRow, Party } from "./primitives";
import { AssetIcon } from "./TokenIcon";
import { t, translate } from "../core/i18n";

const TONE_TEXT = {
    neutral: "text-text-2",
    muted: "text-text-3",
    accent: "text-accent-text",
    ok: "text-ok",
    warn: "text-warn",
    danger: "text-danger",
};

/** 一笔交易。整行进详情页。已确认是常态，不显示标签，异常才显示。compact 用在没有按天分组的地方，时间带日期。 */
export function FeedItem({ row, compact }: { row: FeedRow; compact?: boolean }) {
    const time = row.time === null ? t("等待打包") : compact ? timeLabel(row.time) : minuteLabel(row.time);
    const flagged = row.tone !== "ok";
    return (
        <ListRow
            link={{ to: row.href }}
            label={`${row.title} ${row.amount}`}
            icon={<AssetIcon entry={row.entry} network={row.network} size={32} />}
            title={row.title}
            sub={
                <>
                    <span className="flex min-w-0 gap-s2">
                        <span className="truncate">
                            <Party text={row.party} />
                        </span>
                        {row.note ? (
                            <span className={`truncate ${TONE_TEXT[row.noteTone]}`}>{translate(row.note)}</span>
                        ) : null}
                    </span>
                    {row.remark ? <span className="block truncate text-text-2">{row.remark}</span> : null}
                </>
            }
            badge={flagged && !compact ? <Chip tone={row.tone}>{row.status}</Chip> : null}
            value={
                <>
                    <span className={`text-t3 ${row.direction === "in" ? "text-ok" : ""}`}>{row.amount}</span>
                    {/* 放不下标签时，状态写在时间的位置。 */}
                    {flagged ? (
                        <span className={`text-t1 ${TONE_TEXT[row.tone]} ${compact ? "" : "@split:hidden"}`}>
                            {row.status}
                        </span>
                    ) : null}
                    {flagged && compact ? null : (
                        <span className={`text-t1 text-text-3 ${flagged ? "hidden @split:block" : ""}`}>{time}</span>
                    )}
                </>
            }
        />
    );
}
