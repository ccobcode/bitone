import { Navigate, Outlet, Route, Routes } from "react-router";
import { useApp } from "./data/app";
import Assets from "./pages/Assets";
import Broadcast from "./pages/Broadcast";
import Contacts from "./pages/Contacts";
import Device from "./pages/Device";
import History from "./pages/History";
import Pair from "./pages/Pair";
import Receive, { ReceiveAddresses } from "./pages/Receive";
import Send from "./pages/Send";
import Settings from "./pages/Settings";
import Sign from "./pages/Sign";
import Welcome from "./pages/Welcome";
import { Shell } from "./ui/Shell";

/** 没有任何钱包时回到 W0。 */
function RequireWallet() {
    const { wallets } = useApp();
    return wallets.length > 0 ? <Outlet /> : <Navigate to="/welcome" replace />;
}

export function App() {
    return (
        <Routes>
            <Route path="welcome" element={<Welcome />} />
            <Route element={<Shell />}>
                <Route element={<RequireWallet />}>
                    <Route index element={<Assets />} />
                    <Route path="receive" element={<Receive />} />
                    <Route path="receive/addresses" element={<ReceiveAddresses />} />
                    <Route path="device" element={<Device />} />
                    <Route path="send" element={<Send />} />
                    <Route path="sign" element={<Sign />} />
                    <Route path="broadcast" element={<Broadcast />} />
                    <Route path="history" element={<History />} />
                    <Route path="contacts" element={<Contacts />} />
                </Route>
                <Route path="pair" element={<Pair />} />
                <Route path="settings" element={<Settings />} />
            </Route>
            <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
    );
}
