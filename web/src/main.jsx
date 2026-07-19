// Entry point for the setup web app. Modular ES-module structure:
//   i18n · mock · api · ui · sensors · wizard · integration · diagnostics ·
//   dashboard · review · app. vite-plugin-singlefile inlines everything into ONE HTML
//   file served by the ESP32 from flash.
import { h, render } from 'preact';
import './tokens.css';
import './app.css';
import './table.css';
import './style-variants.css';
import { App } from './app.jsx';

render(h(App), document.getElementById('root'));
