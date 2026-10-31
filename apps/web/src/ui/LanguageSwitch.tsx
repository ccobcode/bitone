import { langChoice, LANGUAGES, setLang, t, type LangChoice } from "../core/i18n";
import { Picker } from "./Picker";

/** 切换时重新加载页面.未配对时这是唯一需要的设置,所以欢迎页也有. */
export function LanguageSwitch() {
    const current = langChoice();
    const options = [{ code: "auto", name: t("自动") }, ...LANGUAGES];
    return (
        <Picker
            id="language"
            look="filter"
            label={t("界面语言")}
            value={options.find((item) => item.code === current)?.name}
            items={options.map((item) => ({ key: item.code, node: item.name, selected: item.code === current }))}
            onPick={(code) => {
                if (code === current) return;
                setLang(code as LangChoice);
                location.reload();
            }}
        />
    );
}
