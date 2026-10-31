import { lang, setLang, t, type Lang } from "../core/i18n";
import { Segmented } from "./primitives";

/** 切换时重新加载页面。未配对时这是唯一需要的设置，所以欢迎页也有。 */
export function LanguageSwitch() {
    return (
        <Segmented<Lang>
            label={t("界面语言")}
            value={lang()}
            options={[
                { value: "zh", label: "中文" },
                { value: "en", label: "English" },
            ]}
            onChange={(next) => {
                setLang(next);
                location.reload();
            }}
        />
    );
}
