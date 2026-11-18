# Threat model

What POP lets in, from where; where its secrets live; and what anyone
standing at the kiosk can see and do. Every claim names the test that would
fail if it stopped being true. A claim with no such test is listed under
[Gaps](#gaps), not made here.

`crates/pop-server/tests/threat_model.rs` reads this file. Every test named
here (a Rust file and the test's name, or a web test file and the test's
title in quotes) must exist, so a renamed or deleted test cannot leave a
claim here that nothing enforces.

## Who POP defends against

- **Anyone on the internet**, if POP is reachable from it, for example
  through a reverse proxy for watching from home.
- **Anyone in the space**: members and visitors on its network, and anyone
  who can touch the kiosk.
- **A signed-in member** reaching for an admin's powers.

POP does not defend against someone with a shell on its machine, or with
its configuration or environment. Those hold the keys to everything below.

## Local and remote

"Watch from anywhere, control only at the space" is enforced by the server,
not by the screens (#7).

**How a request is judged.**
- A request is **local** if it comes from this machine (loopback) or from
  `access.local_networks`. Anything else is **remote**.
  `crates/pop-server/src/access.rs::tests::this_machine_is_local`,
  `crates/pop-server/src/access.rs::tests::the_spaces_network_is_local_and_nowhere_else_is`.
- With no peer address at all, a request is remote: the check fails
  closed. `crates/pop-server/src/access.rs::tests::no_peer_address_is_remote`,
  `crates/pop-server/tests/access.rs::a_local_only_route_refuses_a_request_with_no_peer_address`.
- An IPv4 address carried in IPv6 is matched as IPv4.
  `crates/pop-server/src/access.rs::tests::an_ipv4_address_carried_in_ipv6_matches_its_network`.

**Proxies.** Forwarding headers (`Forwarded`, `X-Forwarded-For`) are
believed only from `access.trusted_proxies`.
- From any other peer they make the request remote.
  `crates/pop-server/src/access.rs::tests::forwarding_headers_from_an_untrusted_peer_make_a_request_remote`,
  `crates/pop-server/tests/access.rs::a_local_only_route_refuses_what_an_unnamed_proxy_passes_on`.
- From a trusted proxy, the client is the rightmost address that no trusted
  proxy added. Addresses to its left were written by the client and are
  ignored.
  `crates/pop-server/src/access.rs::tests::a_trusted_proxy_passes_on_where_the_request_began`,
  `crates/pop-server/src/access.rs::tests::a_client_cannot_claim_to_be_local_through_a_trusted_proxy`,
  `crates/pop-server/src/access.rs::tests::a_chain_of_trusted_proxies_is_walked_back_to_the_client`.
- A chain of nothing but trusted proxies is remote, and so is a header that
  cannot be read.
  `crates/pop-server/src/access.rs::tests::a_chain_of_nothing_but_trusted_proxies_is_remote`,
  `crates/pop-server/src/access.rs::tests::a_forwarding_header_that_cannot_be_read_is_remote`.
- A request carrying both headers is remote, because a client could have
  written either one. Before this was fixed, a client could pass as local
  through a proxy that set `Forwarded` and passed on `X-Forwarded-For`.
  `crates/pop-server/src/access.rs::tests::a_request_with_both_forwarding_headers_is_remote`,
  `crates/pop-server/tests/access.rs::a_local_only_route_refuses_a_client_that_forges_one_forwarding_header_beside_another`.

**What only the space can do.**
- No route that controls a printer is reachable from outside the space. The
  check enumerates every route in the real table, and it is self-tested
  with a deliberately mis-marked route.
  `crates/pop-server/tests/access.rs::no_route_that_controls_a_printer_is_reachable_from_outside`,
  `crates/pop-server/tests/access.rs::the_check_fails_a_printer_route_marked_reachable_from_anywhere`.
- Every local-only route in the real table refuses a remote request.
  `crates/pop-server/tests/access.rs::every_local_only_route_of_the_real_table_refuses_a_remote_request`.
- Today those routes are:
  - adding and declining a printer from BAM!
    (`crates/pop-server/tests/tower.rs::which_printers_the_tower_runs_is_decided_only_at_the_space`);
  - entering a Bambu access code
    (`crates/pop-server/tests/printers.rs::a_code_is_entered_only_from_inside_the_space`);
  - setting, clearing and testing the Discord webhook
    (`crates/pop-server/tests/notify.rs::where_notices_go_is_set_only_at_the_space_and_only_by_an_admin`).
- No route to start, pause, stop or resume a print exists yet. When one
  does, the enumeration above covers it from its first commit.

**What anyone can reach, from anywhere.** Health, status, the live stream
(`/api/events`), signing in, and the published guides.

The live stream carries printer names, job file names, error text and group
notices, and every field it carries is pinned: a new one fails the test until
someone has decided it may go out.
`crates/pop-server/tests/public_stream.rs::the_public_stream_carries_only_reviewed_fields`.
Whose print is whose goes out to the public only as `[public] owner_names`
says: off (the default), nobody is named, in a printer's job or a notice; on,
first names only. A screen signed in, or on the space's own network (the
kiosk), sees names either way; a session cookie that is not a session counts
for nothing.
`crates/pop-server/tests/public_stream.rs::with_owner_names_off_the_public_sees_nobody_named`,
`crates/pop-server/tests/public_stream.rs::with_owner_names_on_the_public_sees_first_names_only`,
`crates/pop-server/tests/public_stream.rs::signed_in_or_at_the_space_names_are_seen_either_way`,
`crates/pop-server/tests/public_stream.rs::a_session_that_is_not_one_is_the_public`,
`crates/pop-server/tests/public_stream.rs::what_a_reconnecting_screen_missed_is_named_as_it_may_see`. Nothing public changes
anything except signing in, and that check is self-tested too.
`crates/pop-server/tests/access.rs::no_public_route_changes_anything_but_signing_in`,
`crates/pop-server/tests/access.rs::the_check_fails_a_public_route_that_writes`.

**Roles.** Every route refuses whoever its table entry does not name:
- anonymous callers get 401 on member and admin routes;
- members get 403 on admin routes.

`crates/pop-server/tests/users.rs::every_route_refuses_whoever_it_does_not_name`,
`crates/pop-server/tests/users.rs::every_admin_route_is_under_api_admin_and_nothing_else_is`.

**An admin signed in from outside the space can:**
- manage accounts: add them, reset passwords, change roles and deactivate;
- write and publish guides;
- read the audit log.

They cannot control a printer, enter an access code, or change where
notices go.

## Signing in

- **Passwords are stored as Argon2id hashes, never in plain text.** Session
  tokens are stored only as SHA-256 hashes.
  `crates/pop-server/tests/users.rs::the_database_holds_neither_password_nor_session_token`.
- **A wrong password and an unknown username get the same answer.**
  `crates/pop-server/tests/users.rs::a_wrong_password_and_an_unknown_user_get_the_same_answer`.
- **The session cookie** is `HttpOnly` and `SameSite=Lax`.
  `crates/pop-server/tests/users.rs::signing_in_starts_a_session_that_says_who_you_are`.
- **Sessions end:**
  - when they expire
    (`crates/pop-server/tests/users.rs::an_expired_session_signs_nobody_in`);
  - on sign-out
    (`crates/pop-server/tests/users.rs::signing_out_ends_the_session`);
  - when the account's password is reset
    (`crates/pop-server/tests/users.rs::a_password_reset_works_and_signs_out_whoever_knew_the_old_one`);
  - when the account is deactivated
    (`crates/pop-server/tests/users.rs::a_deactivated_account_is_signed_out_and_cannot_sign_in_until_reactivated`,
    `crates/pop-server/tests/users.rs::a_session_of_a_deactivated_account_is_refused_even_if_it_survived`).
- **The last active admin cannot be demoted or deactivated.**
  `crates/pop-server/tests/users.rs::an_admin_can_change_a_role_but_not_remove_the_last_admin`,
  `crates/pop-server/tests/users.rs::the_last_active_admin_cannot_be_deactivated_or_demoted`.
- **The first admin is made only from the command line** on POP's machine.
  `crates/pop-server/tests/cli.rs::create_admin_makes_an_admin_who_can_sign_in`.
- **Cross-site requests.** A write must be `Content-Type: application/json`,
  which a plain form on another site cannot send. Together with
  `SameSite=Lax` and no CORS, that is POP's defence against cross-site
  request forgery.
  `crates/pop-server/tests/users.rs::a_request_that_changes_something_must_be_json`.

## Secrets

| Secret | Where it lives | How it is kept |
|---|---|---|
| BAM! API token | `POP_BAM_TOKEN` (or `[bam] token`) | Never in the database or a backup |
| Secrets key | `POP_SECRETS_KEY` (or `[secrets] key`), 32 bytes | Never in the database or a backup |
| Bambu access codes | Database, `printer_secrets` | Sealed with the secrets key |
| Discord webhook URL | Database, `sealed_settings` | Sealed with the secrets key |
| go2rtc API password | `POP_GO2RTC_PASSWORD` (or `[go2rtc] password`) | Never in the database or a backup |
| Camera addresses, which may hold a password | BAM!'s `camera` field, and go2rtc's configuration file | Plain text in both; never shown or logged by POP (gap 17) |
| Passwords | Database | Argon2id hash only |
| Session tokens | Database | SHA-256 hash only |

**The secrets key** must be exactly 32 bytes.
`crates/pop-server/src/config.rs::tests::a_secrets_key_must_be_32_bytes`.

**Bambu access codes.**
- Sealed with ChaCha20-Poly1305 under a fresh nonce, bound to their printer.
  One sealed for one printer does not open as another's.
  `crates/pop-server/tests/printers.rs::the_code_is_kept_sealed_and_opens_with_its_key_only`.
- Without a secrets key, POP refuses to keep a code rather than storing it
  in plain text.
  `crates/pop-server/tests/printers.rs::without_a_secrets_key_pop_refuses_to_keep_the_code`.
- No API returns a code, and its audit entry does not hold it.
  `crates/pop-server/tests/printers.rs::entering_an_access_code_is_recorded_without_the_code`.

**The Discord webhook.**
- Sealed the same way, and shown again only masked.
  `crates/pop-server/tests/notify.rs::a_saved_webhook_is_sealed_and_never_shown_in_full`,
  `crates/pop-server/src/notify.rs::tests::a_saved_webhook_is_shown_without_its_token`.
- Never written to the log, and kept out of its audit entries.
  `crates/pop-server/tests/notify.rs::the_token_never_reaches_the_log`,
  `crates/pop-server/tests/notify.rs::setting_clearing_and_testing_are_recorded_without_the_url`.
- Without a key it is not kept.
  `crates/pop-server/tests/notify.rs::without_a_secrets_key_no_webhook_is_kept`.
- Only a Discord URL is accepted, and a notice can ping nobody.
  `crates/pop-server/tests/notify.rs::a_webhook_that_is_not_discords_is_refused_by_name`,
  `crates/pop-server/tests/notify.rs::a_notice_is_one_plain_sentence_that_can_ping_nobody`.

**The BAM! token** is sent on every request to BAM!, and `/healthz` reports
on BAM! without naming its address.
`crates/pop-integrations/tests/bam.rs::every_request_carries_the_token_and_asks_only_what_was_recorded`,
`crates/pop-server/tests/printers.rs::healthz_reports_bam_by_name_and_without_detail`.

**Cameras and go2rtc** (docs/integrations/go2rtc.md).
- A camera's address is never shown to admins, and never in an error POP
  makes about a request to go2rtc.
  `crates/pop-server/tests/cameras.rs::each_cameras_health_is_what_go2rtc_shows_and_admins_see_it`,
  `crates/pop-integrations/tests/go2rtc.rs::an_error_never_repeats_a_cameras_address`.
- Only `rtsp`, `rtsps`, `http` and `https` cameras are taken from BAM!; and
  go2rtc's own refusal of a source that runs a program is kept, in its words.
  `crates/pop-integrations/tests/go2rtc.rs::a_source_go2rtc_refuses_is_refused_in_its_words`.
- POP removes every `pop_` stream no printer wants, so a camera's address
  does not stay in go2rtc after its printer leaves, and leaves every other
  stream alone.
  `crates/pop-server/tests/cameras.rs::streams_that_are_not_pops_are_left_alone_and_stale_ones_of_pops_go`,
  `crates/pop-server/tests/cameras.rs::a_printer_that_leaves_the_tower_leaves_go2rtc`.
- A wrong go2rtc password is said as such on `/healthz`, and nothing goes in.
  `crates/pop-server/tests/cameras.rs::a_wrong_go2rtc_password_is_said_and_cameras_read_unknown`.
- Watching is for members, and only a tower printer's camera: no go2rtc
  stream can be named.
  `crates/pop-server/tests/cameras.rs::only_members_watch_and_only_a_tower_printers_camera`.
- Watching is read-only: nothing a viewer sends reaches go2rtc but a codec
  list POP has checked, and go2rtc's own words about a camera never reach the
  viewer.
  `crates/pop-server/tests/cameras.rs::a_member_watches_an_h264_camera_and_nothing_else_they_send_reaches_go2rtc`,
  `crates/pop-server/tests/cameras.rs::a_codec_list_that_is_not_one_is_refused_before_go2rtc_hears_it`,
  `crates/pop-server/tests/cameras.rs::what_go2rtc_says_about_a_camera_never_reaches_a_viewer`.
- A Bambu printer's own camera is reached with its certificate pinned, as
  its MQTT is: go2rtc, which would check none, gets the RTSPS camera only
  through POP's relay, and a different certificate stops the relay.
  `crates/pop-server/tests/bambu_cameras.rs::an_x1s_camera_is_played_by_go2rtc_through_the_pinned_relay`,
  `crates/pop-server/tests/bambu_cameras.rs::an_x1_with_another_certificate_gets_nothing_and_admins_are_told_why`,
  `crates/pop-printers/tests/bambu_camera.rs::the_relay_passes_nothing_to_a_camera_with_another_certificate`.
- The port-6000 camera stream is read by a decoder fuzzed to never panic or
  hold more than one picture.
  `crates/pop-printers/tests/bambu_camera.rs::the_decoder_survives_whatever_a_camera_sends`.
- No more than 16 watch at once.
  `crates/pop-server/tests/cameras.rs::no_more_than_the_limit_watch_at_once`.

**Backups** hold the database, so they include the sealed secrets and the
hashes, but never the secrets key or the BAM! token. A backup alone opens
none of the sealed secrets. Restoring refuses an archive that has been
altered or that would write outside the data directory.
`crates/pop-server/tests/backup.rs::an_altered_file_is_refused_by_name`,
`crates/pop-server/tests/backup.rs::a_path_that_leaves_the_data_directory_is_refused_before_it_is_written`,
`crates/pop-server/tests/backup.rs::a_restore_never_overwrites_a_database`.

## Printers

**Bambu printers** speak TLS with a self-signed certificate. POP pins each
printer's certificate the first time it connects. If the certificate later
changes, POP refuses the connection, shows the printer as not just offline
but changed, and never sends the access code to the new certificate.
`crates/pop-printers/tests/bambu.rs::the_certificate_is_pinned_on_first_contact`,
`crates/pop-printers/tests/bambu.rs::a_different_certificate_is_refused_and_shown_not_just_offline`,
`crates/pop-printers/tests/bambu.rs::pins_are_kept_per_printer`,
`crates/pop-server/tests/run.rs::a_bambu_pin_outlives_a_restart`.

## The kiosk

Anyone in the space can touch the kiosk, so it is treated as public.
- It shows what `/api/events` and the published guides show anyone, and
  calls nothing else.
- It never resumes a print.
  `web/src/lib/tower/HandView.test.ts › "never offers to resume a print that stopped"`.
- Its "needs a hand" view returns to the printers after two minutes
  untouched, but not while someone is using it.
  `web/src/lib/tower/HandView.test.ts › "goes back when asked, and when left alone"`,
  `web/src/lib/tower/HandView.test.ts › "is not sent back while someone is using it"`.

## Guides

- **Only an admin writes or reads drafts.** A stranger finds no draft, and
  no draft's image.
  `crates/pop-server/tests/guides.rs::only_an_admin_writes_or_reads_drafts`,
  `crates/pop-server/tests/public_guides.rs::a_stranger_reads_a_published_guide_and_finds_no_draft_or_its_image`,
  `crates/pop-server/tests/public_guides.rs::the_public_list_has_only_published_guides`,
  `crates/pop-server/tests/error_guides.rs::a_link_to_a_draft_is_never_shown_to_anyone`.
- **A public guide cannot be changed through its public address.**
  `crates/pop-server/tests/public_guides.rs::a_public_guide_cannot_be_changed_through_its_public_address`.
- **Images are PNG, JPEG or WebP, checked by their bytes, not their name.**
  SVG and HTML are refused, and images are served with `nosniff`.
  `crates/pop-server/tests/guides.rs::an_image_that_is_not_one_is_refused`,
  `crates/pop-server/tests/guides.rs::an_image_is_kept_beside_the_print_files_and_a_step_may_use_it`.
- **Search engines index nothing**, unless the space allows its public
  guides to be indexed.
  `crates/pop-server/tests/server.rs::no_screen_is_indexed_but_the_public_guides_when_allowed`,
  `crates/pop-server/tests/public_guides.rs::the_license_is_shown_and_indexing_is_off_unless_allowed`.

## The audit log

Only an admin reads it. Every route that changes something is listed in
`crates/pop-server/tests/audit.rs`, and the list must equal the route table.
`crates/pop-server/tests/audit.rs::every_write_route_is_in_this_file`.

Each route has its own test that it is recorded, for example:
- `crates/pop-server/tests/audit.rs::a_failed_sign_in_is_recorded_without_the_password`
- `crates/pop-server/tests/tower.rs::adding_and_declining_are_recorded`
- `crates/pop-server/tests/guides.rs::writing_editing_publishing_and_renaming_are_recorded`

## Remote access

docs/remote-access.md puts POP behind a proxy for watching from outside
(#93). Its example, docs/remote/nginx.conf, forwards every route POP allows
from anywhere and no route kept to the space, refuses API paths POP does not
have, and sets the forwarding header rather than adding to one a client
sent. `crates/pop-server/tests/remote_access.rs::the_example_forwards_what_pop_allows_from_anywhere_and_nothing_else`,
`crates/pop-server/tests/remote_access.rs::the_check_fails_an_example_that_forwards_too_much_or_forges_nothing`.
The test reads the file as nginx chooses locations; nginx itself is not run
in POP's tests. POP's own refusal of local-only routes from outside does not
depend on the proxy.

## Flagging a problem

A member watching can flag that something looks wrong (#63), and POP tells
the group chat (docs/integrations/discord.md). It never touches the printer:
the printer is asked nothing new.
`crates/pop-server/tests/report.rs::a_report_never_touches_the_printer`.
Only members signed in can flag, a limited number of times an hour, and a
printer is not posted about again for ten minutes; what they write is posted
as plain text that can ping nobody.
`crates/pop-server/tests/report.rs::only_members_flag_and_only_a_printer_on_the_tower`,
`crates/pop-server/tests/report.rs::a_printer_flagged_a_moment_ago_is_not_posted_about_again`,
`crates/pop-server/tests/report.rs::what_a_member_writes_is_posted_as_plain_text`.

## Feedback

Feedback is filed as issues on a public repository (#96,
docs/integrations/github-feedback.md), so what a report carries is public.

- Only roles named in `[feedback] roles` (admins, by default) may file it.
  `crates/pop-server/tests/feedback.rs::only_those_holding_feedback_file_can_file_and_see_the_button`.
- The context is scrubbed on the server of tokens, cookies, `Authorization`
  values and query strings, each kind caught on its own; the printers'
  states in it name nobody.
  `crates/pop-server/tests/feedback.rs::no_token_cookie_or_authorization_reaches_the_issue`,
  `crates/pop-server/tests/feedback.rs::each_kind_of_token_is_caught_by_itself`.
- The screenshot is a PNG with its metadata taken out.
  `crates/pop-server/tests/feedback.rs::metadata_goes_and_the_picture_stays`.
- The GitHub App's private key stays on the server; the browser never sees
  it. A report names who filed it, by name, as the issue says it will.

- The screenshot is redacted before it becomes an image, on a clone of the
  page: what screens mark with `use:sensitive` (members' names, usernames,
  job file names, notices naming owners) becomes a labelled block; form
  fields are emptied; camera pictures become grey boxes; and a backstop
  takes emails, long numbers, UUIDs and tokens out of every text left, each
  kind caught on its own. The page on screen is untouched.
  `web/src/lib/feedback/redact.test.ts › "leaves nothing marked in the clone, and the page as it was"`,
  `web/src/lib/feedback/redact.test.ts › "catches what was not marked: an email, a card number, a token"`,
  `web/src/lib/feedback/redact.test.ts › "catches each kind of secret by itself"`.
- No screen renders a member's details outside `use:sensitive`, so a new
  one cannot forget to; the check is fed a screen that forgot.
  `web/src/lib/feedback/structure.test.ts › "are always inside use:sensitive"`,
  `web/src/lib/feedback/structure.test.ts › "fails a screen that renders a name unmarked"`.
  It sees member details by how templates reach them; one reached through
  a variable named for something else is not seen, and the backstop and the
  person looking at the picture before sending stand behind that.

## Gaps

Each of these is true of POP today, or untested, and none is claimed above.

**Reach.**
1. **Kiosk sessions do not exist yet (#12).** "Local" means only the
   network. Anyone on the space's network, not just the kiosk, can use
   local-only routes once signed in as an admin. Tracked in #131.
2. **Anything that reaches POP from its own machine is local.** That
   includes an SSH tunnel or a tunnel daemon that adds no forwarding header.
   For remote watching, use a proxy that sets one forwarding header, and
   name it in `trusted_proxies`. Tracked in #132.
3. **Three local-only routes have no test that fails if they are re-marked
   as reachable from anywhere:** reading the notification settings, clearing
   the webhook, and testing it. The enumeration test covers only routes that
   control printers. Tracked in #133.
4. **Closed (#134).** The live stream's fields are pinned, and owners'
   names on it follow `[public] owner_names`; see "What anyone can reach".

**Signing in.**
5. **There is no rate limit or lockout on signing in.** Every failed try
   costs an Argon2 hash and adds an audit row. Tracked in #135.
6. **The session cookie is not `Secure` (#93), and lasts 30 days.** Expired
   sessions are never deleted. A sign-in at the kiosk's browser lasts until
   someone signs out; there is no idle sign-out. Tracked in #136.
7. **The Argon2 parameters are the library's defaults**, and untested.
   **Equal timing** for unknown, deactivated and wrong-password sign-ins is
   intended, and untested. Tracked in #137.
8. **POP sends no `Content-Security-Policy`, frame or HSTS headers.** Guide
   text is plain text that the screens escape, but no test says so. Tracked in #138.

**Secrets and data.**
9. **The data directory, database and backups are written with default
   file permissions.** Tracked in #139.
10. **POP's configuration derives `Debug` with the BAM! token, the go2rtc
    password and the secrets key in it.** Nothing prints it today, but nothing stops it. No test
    keeps the BAM! token, cookies or request headers out of the log. Tracked in #140.
11. **A sealed setting (the webhook) is not tested against being moved to
    another setting's row**, as access codes are. Tracked in #141.

**Printers, the audit log and uploads.**
12. **There is no way yet to accept a Bambu printer's legitimately changed
    certificate.** `docs/printers/bambu.md` says an admin confirms it. Tracked in #142.
13. **TLS verification to BAM! and Discord is the library default, and
    untested.** BAM! may be configured over plain `http`. Moonraker defaults
    to plain `ws`. Tracked in #143.
14. **Some changes are not recorded in the same transaction as their audit
    entry:** access codes, the webhook, guides and error-guide links. If
    the audit write fails, the change stands without its entry. Tracked in #144.
15. **A guide image's 5 MB limit is likely unreachable**, because the
    server's default request size is 2 MB. Tracked in #145.

**Cameras.**
16. **POP does not check that go2rtc asks for its password.** go2rtc skips
    it for requests from its own machine unless `local_auth` is on, so any
    program on the box could change its streams. The guide says to turn it
    on; nothing checks. Tracked in #149.
17. **Camera addresses are plain text in BAM! and in go2rtc's
    configuration file**, and a camera's address can hold its password. A
    Bambu X1's access code is among them: go2rtc's address for the relay
    carries it, though the code itself never goes in BAM!.
    POP does not set that file's permissions. Tracked in #150.
