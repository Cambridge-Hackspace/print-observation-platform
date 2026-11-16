import { mount } from 'svelte';
// Lexend, bundled into the app rather than fetched: the kiosk may have no
// internet (#48).
import '@fontsource/lexend/400.css';
import '@fontsource/lexend/500.css';
import '@fontsource/lexend/600.css';
import './lib/design/tokens.css';
import App from './App.svelte';
import Gallery from './Gallery.svelte';
import Tower from './lib/tower/Tower.svelte';

const target = document.getElementById('app')!;
// /design shows the design system (#48), /kiosk the tower's home screen
// (#49); everything else is the app.
const screens: Record<string, typeof App> = { '/design': Gallery, '/kiosk': Tower };
mount(screens[location.pathname] ?? App, { target });
