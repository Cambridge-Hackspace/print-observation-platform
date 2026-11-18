import { mount } from 'svelte';
// Lexend, bundled into the app rather than fetched: the kiosk may have no
// internet (#48).
import '@fontsource/lexend/400.css';
import '@fontsource/lexend/500.css';
import '@fontsource/lexend/600.css';
import './lib/design/tokens.css';
import Gallery from './Gallery.svelte';
import Tower from './lib/tower/Tower.svelte';
import GuidePage from './lib/guides/GuidePage.svelte';
import GuidesIndex from './lib/guides/GuidesIndex.svelte';
import Admin from './lib/admin/Admin.svelte';
import Phone from './lib/phone/Phone.svelte';
import Feedback from './lib/feedback/Feedback.svelte';
import { watch } from './lib/feedback/context';

// What a feedback report carries of what went wrong is kept from the start (#96).
watch();

const target = document.getElementById('app')!;
const path = location.pathname.replace(/\/+$/, '') || '/';
// A guide's slug from /guides/{slug} or /kiosk/guides/{slug}.
const guide = /^\/(kiosk\/)?guides\/([a-z0-9-]+)$/.exec(path);
if (guide) {
  // A guide, for anyone (#94), or at the kiosk (#56).
  mount(GuidePage, { target, props: { slug: guide[2]!, kiosk: guide[1] !== undefined } });
} else {
  // /design shows the design system (#48), /kiosk the tower's home screen
  // (#49), /guides every published guide (#94), /admin the admin desk (#86);
  // everything else is the phone app (#60), which is what the public address
  // and the kiosk's QR codes open.
  const screens: Record<string, typeof Phone> = {
    '/design': Gallery,
    '/kiosk': Tower,
    '/guides': GuidesIndex,
    '/admin': Admin,
  };
  mount(screens[path] ?? Phone, { target });
}

// The feedback button, on every screen, for whoever holds feedback.file (#96).
const feedback = document.createElement('div');
document.body.appendChild(feedback);
mount(Feedback, { target: feedback });
