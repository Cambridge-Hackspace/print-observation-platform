// `use:sensitive` (#96): marks an element whose content is a member's
// details, so a feedback screenshot never shows it.
//
//   <span use:sensitive={'name'}>{me.name}</span>
//
// Every component showing a member's name, email, username, card or a job's
// file name uses it (feedback/structure.test.ts holds them to it). It only
// sets an attribute: redact.ts, run on the clone a screenshot is made from,
// does the rest. Nothing on screen changes.

import { ATTRIBUTE, type SensitiveKind } from './redact';

export function sensitive(node: HTMLElement, kind: SensitiveKind = 'name') {
  node.setAttribute(ATTRIBUTE, kind);
  return {
    update(next: SensitiveKind = 'name') {
      node.setAttribute(ATTRIBUTE, next);
    },
    destroy() {
      node.removeAttribute(ATTRIBUTE);
    },
  };
}
