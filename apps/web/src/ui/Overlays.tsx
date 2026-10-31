import { useSearchParams } from "react-router";
import { ExportDialog, ImportDialog } from "../pages/Backup";
import PairDialog from "../pages/Pair";
import ReceiveDialog from "../pages/Receive";

/** 网址上的浮层。页面不动，关掉回到原处。 */
export function Overlays() {
    const [params] = useSearchParams();
    if (params.has("connect")) return <PairDialog />;
    if (params.has("import")) return <ImportDialog />;
    if (params.has("export")) return <ExportDialog />;
    if (params.has("receive")) return <ReceiveDialog />;
    return null;
}
