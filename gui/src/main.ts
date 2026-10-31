import { mount } from "svelte";
import "./app.css";
// Sets the theme on <html> before the first paint.
import "./lib/theme.svelte";
import App from "./App.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("missing #app");

export default mount(App, { target });
