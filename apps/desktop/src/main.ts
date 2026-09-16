import { mount } from "svelte";
import App from "./App.svelte";
import { catchWindowErrors } from "./journal";
import "./ui/tokens.css";

// До монтирования: ошибка в нём самом — это пустое окно, и тогда лог остаётся
// единственным, что о ней скажет.
catchWindowErrors();

export default mount(App, { target: document.getElementById("app")! });
