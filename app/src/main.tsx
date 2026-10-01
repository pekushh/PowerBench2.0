import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initWindow } from "./windowState";

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("Не найдено приложение: в index.html нет <div id=\"root\">.");
}

// Контракт окна применяется до первого кадра: иначе интерфейс на миг
// показывается в размере из конфига, а потом прыгает к сохранённому.
void initWindow();

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);