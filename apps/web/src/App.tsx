import { Navigate, Outlet, Route, Routes } from "react-router";
import { useApp } from "./data/app";
import AssetDetail from "./pages/AssetDetail";
import Assets from "./pages/Assets";
import Contacts from "./pages/Contacts";
import Device from "./pages/Device";
import History, { Unsent } from "./pages/History";
import Raw from "./pages/Raw";
import Send from "./pages/Send";
import Settings from "./pages/Settings";
import Sign from "./pages/Sign";
import TxDetail from "./pages/TxDetail";
import Welcome from "./pages/Welcome";
import { Shell } from "./ui/Shell";

/** 没有任何钱包时回到 W0.这时除了欢迎页,其他页面都没有意义. */
function RequireWallet() {
    const { wallets } = useApp();
    return wallets.length > 0 ? <Outlet /> : <Navigate to="/welcome" replace />;
}

export function App() {
    return (
        <Routes>
            <Route path="welcome" element={<Welcome />} />
            <Route element={<RequireWallet />}>
                <Route element={<Shell />}>
                    <Route index element={<Assets />} />
                    <Route path="assets/:asset" element={<AssetDetail />} />
                    <Route path="history" element={<History />} />
                    <Route path="history/unsent" element={<Unsent />} />
                    <Route path="history/:id" element={<TxDetail />} />
                    <Route path="history/:network/:hash" element={<TxDetail />} />
                    <Route path="contacts" element={<Contacts />} />
                    <Route path="device" element={<Device />} />
                    <Route path="settings" element={<Settings />} />
                    <Route path="raw" element={<Raw />} />
                    <Route path="send" element={<Send />} />
                    <Route path="sign/:id" element={<Sign />} />
                </Route>
            </Route>
            <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
    );
}
