# Printers behind OctoPrint

POP watches printers run by OctoPrint (#29), such as an Ender 3 on an
OctoPi: whether each is idle, printing, paused, done or needs a hand, how
far through its print, the time left, and the file's name. Sending prints
to them comes with #45.

## In BAM!

The printer's `connection` field is `octoprint`, and its `address` is
OctoPrint's address, as a browser opens it: `http://octopi.local` or
`http://192.168.1.53` (docs/integrations/bam.md). Use `https://` if
OctoPrint is served that way.

## Its API key

POP needs an API key, and keeps it sealed, never in BAM!. In OctoPrint,
make one under **Settings → Application Keys** for a user that may watch
and print but not administer (docs/threat-model.md, gap 20). Then, on the
admin desk, under **Kept in POP**, choose **Enter** beside "API key".

POP sends the key only to the address BAM! had when it was entered. If the
address changes in BAM!, the printer reads offline, POP's log says why, and
the desk asks for the key again.

## When it reads offline

- **The printer is not connected to OctoPrint.** OctoPrint is up, but its
  printer is off, unplugged or not connected in OctoPrint's own Connection
  panel. POP keeps asking, and reads it again as soon as it is connected.
- "OctoPrint asks for an API key" or "OctoPrint refused POP's API key", in
  `journalctl -u pop`: enter it, or a new one, on the desk.
- "OctoPrint at http://…: …": the OctoPi is off, or not at that address.

## What POP shows

| OctoPrint                     | On POP's screens                 |
| ----------------------------- | -------------------------------- |
| printer not connected         | offline                          |
| error                         | needs a hand, in OctoPrint's words |
| paused or pausing             | paused                           |
| cancelling                    | free                             |
| printing or finishing         | printing                         |
| its last job finished at 100% | done, the print still on the bed |
| otherwise                     | free                             |

Built against OctoPrint's REST documentation, not yet checked against a
printer of the space's: the first one added is the check.
