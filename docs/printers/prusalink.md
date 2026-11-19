# Prusa printers, through PrusaLink

POP watches Prusa printers through PrusaLink (#30): whether each is idle,
printing, paused, done or needs a hand, how far through its print, the time
left, and the file's name. Sending prints to them comes with #45.

## What the printer needs

- **PrusaLink switched on.** On an MK4, XL or MINI+, it is in the printer's
  own network settings. On an MK3 or MINI, it runs on a Raspberry Pi
  attached to the printer.
- **A fixed address on the space's network**, by a DHCP reservation, so POP
  finds it after a restart.

## In BAM!

The printer's `connection` field is `prusalink`, and its `address` is
PrusaLink's address, as a browser opens it: `http://192.168.1.52`
(docs/integrations/bam.md).

## Its login

PrusaLink asks for one, and POP keeps it sealed, never in BAM!. On the admin
desk, under **Kept in POP**, choose **Enter** beside "Printer login":

- **On the printer** (MK4, XL, MINI+): the user and password the printer
  shows in its network settings. The user is usually `maker`. POP signs in
  with HTTP Digest, which never sends the password itself over the network.
- **On a Raspberry Pi**: its API key, from PrusaLink's own settings page.

POP signs in only at the address BAM! had when the login was entered. If
the address changes in BAM!, the printer reads offline, POP's log says why,
and the desk asks for the login again.

## When it reads offline

`journalctl -u pop` says why, in plain words:

- "PrusaLink asks for a login": none is entered yet.
- "PrusaLink refused POP's login": the user or password is wrong, or the
  printer's password changed.
- "PrusaLink at http://…: …": the printer is off, or not at that address.

## What POP shows

| PrusaLink                    | On POP's screens                      |
| ---------------------------- | ------------------------------------- |
| `IDLE`, `READY`, `STOPPED`   | free                                  |
| `BUSY`                       | printing with a job, free without one |
| `PRINTING`                   | printing                              |
| `PAUSED`                     | paused                                |
| `FINISHED`                   | done, the print still on the bed      |
| `ATTENTION`                  | needs a hand, the print held          |
| `ERROR`                      | needs a hand, in the printer's words  |

Built against PrusaLink's published API description (Prusa-Link-Web), not
yet checked against a printer of the space's: the first one added is the
check.
