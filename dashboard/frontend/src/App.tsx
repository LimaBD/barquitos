import { BrowserRouter, Routes, Route } from 'react-router-dom';
import HomePage from './components/HomePage';
import MapPage from './components/MapPage';
import DbMetricsPage from './components/DbMetricsPage';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<HomePage />} />
        <Route path="/map" element={<MapPage />} />
        <Route path="/metrics" element={<DbMetricsPage />} />
      </Routes>
    </BrowserRouter>
  );
}
