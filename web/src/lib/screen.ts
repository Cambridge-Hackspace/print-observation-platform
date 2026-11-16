// What a screen shows, built from the live stream (#6). A snapshot replaces
// everything; every other event changes the part it names. Screens read this
// state, and never the stream itself.

import type { ServerEvent } from './generated/ServerEvent';
import type { Status } from './generated/Status';
import type { TowerPrinter } from './generated/TowerPrinter';
import type { GroupNotice } from './generated/GroupNotice';

export interface ScreenState {
  status: Status | null;
  /** The tower's printers, by name; empty until the first snapshot. */
  printers: TowerPrinter[];
  /** What the group chat was told about printers that need a hand (#55). */
  told: GroupNotice[];
}

export const empty = (): ScreenState => ({ status: null, printers: [], told: [] });

/** The state after `e`. Pure, so a screen's state is the fold of its stream. */
export function reduce(state: ScreenState, e: ServerEvent): ScreenState {
  switch (e.type) {
    case 'snapshot':
      return { ...empty(), status: e.data.status, printers: e.data.printers, told: e.data.told };
    case 'status':
      return { ...state, status: e.data };
    case 'printers':
      return { ...state, printers: e.data };
    case 'told':
      return { ...state, told: e.data };
  }
}
