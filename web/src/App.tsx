import { BrowserRouter, Route, Routes } from "react-router-dom";
import Home from "./pages/Home";
import ControllerRoom from "./pages/ControllerRoom";
import JoinConsent from "./pages/JoinConsent";
import ParticipantSession from "./pages/ParticipantSession";
import BrowserSession from "./pages/BrowserSession";

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/wait/:token" element={<ControllerRoom />} />
        <Route path="/join/:token" element={<JoinConsent />} />
        <Route path="/session/:token" element={<ParticipantSession />} />
        <Route path="/browser-session/:token" element={<BrowserSession />} />
      </Routes>
    </BrowserRouter>
  );
}
