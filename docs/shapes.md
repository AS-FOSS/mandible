# Shape atlas

`--help` text from real tools repeats a small set of structural shapes: a
three-column options table, an alias run, a wrapped description that starts
with a dash, a modifier table, and so on. This file is the single place that
knowledge lives. Source comments that used to narrate a shape in full should
shrink to a pointer at the matching id below.

One entry per shape. Each entry carries exactly five fields:

- `id`: stable, `S-NNN`, assigned in write order, zero-padded to three
  digits. Ids are permanent and are never renumbered later.
- `looks like`: 2 to 4 lines of real help text, taken verbatim from a
  capture or from the source comment that named the shape.
- `tools`: comma-separated tool names the source material names as
  exhibiting the shape. `unnamed` when the source names none.
- `handling`: what the parser does with the shape, or that the shape is
  refused and why.
- `fleet`: a count and a date when the source material gives one, else
  `not measured`.

A tool exhibiting a shape already in this atlas adds its own name to that
entry's `tools` field and nothing else. It does not get a new entry.

### S-001: usage-line entry point, three shapes

- id: S-001
- looks like: |
      usage: git [-v | --version] [-h | --help] [-C <path>] [-c <name>=<value>]
                 [--exec-path[=<path>]] [--html-path] [--man-path] [--info-path]
      nfsidmap: Usage: nfsidmap [-vh] [-c || [-u|-g|-r key] || -d || -l || [-t timeout] key desc]
- tools: git, nfsidmap, wpa_cli, gh, busctl, journalctl, pidof
- handling: An ordinary usage:/Usage: line opens a usage block anywhere. The C fprintf
  idiom, tool name then ": " then usage:, is a second entry point, recognized
  only once the tool's own name is already known. A bare USAGE heading, or no
  heading at all with the synopsis line itself opening on the tool's own name,
  is a third, narrower entry point used only before the document's real body
  starts. The existence-checking oracle re-exports these same entry-point
  predicates rather than re-deriving a second copy that could drift out of
  sync with a real fix. An operand recovered from this third, unlabelled shape
  still reports as invented today, since the oracle's own synopsis scanner
  recognizes only labelled lines; measured at 3 affected tools
  (dbus-cleanup-sockets, dbus-run-session, lvreduce).
- fleet: 94 tools on one before/after sweep for the unlabelled/fprintf entry points;
  3 tools for the unlabelled-operand reporting gap, not dated

### S-002: escape bytes corrupt heading detection

- id: S-002
- looks like: |
      [0mCommands:
        list       List items
        show       Show one item
- tools: systemd-creds, varlinkctl, systemd-sysext, systemd-confext
- handling: An ANSI reset code glued onto a heading fused with the heading's first
  letters into one alphanumeric run, so the heading-word test never matched.
  mandible_core::strip_escapes now runs once over the whole raw document
  before any layout analysis. systemd-sysext and systemd-confext still fail,
  since their command rows sit under prose with no heading at all.
- fleet: 2 tools recovered on a full-PATH sweep, 0 other flag or subcommand changes,
  2026-08-30

### S-003: colon as the spec/description separator

- id: S-003
- looks like: |
      [-d] [-hr] [-s] [-V] DEVICE
      -d : output debug
      -hr: Set Honor Reservation bit
      -V: print version string then exit
- tools: sg_emc_trespass
- handling: find_colon_separator_gap is a new fallback in find_description_gap, tried
  only after the two-space and equals-token fallbacks find nothing. It
  recognizes a lone spaced colon or a glued trailing colon on an otherwise
  spec-shaped token, never a heading word like Options:.
- fleet: not measured

### S-004: self-closed bracket group before a positional

- id: S-004
- looks like: |
      [-d] [-hr] [-s] [-V] DEVICE
- tools: sg_emc_trespass, scsi_ready, lzgrep, xzgrep, renice, scsi_readcap,
  scsi_start, scsi_stop, scsi_temperature
- handling: extract_positionals no longer reads the token right after a self-closed
  bracket group as that flag's own argument. Recovers DEVICE, sg3-utils's
  device+, PATTERN, priority and pid. Scoped to a tool's one primary labelled
  invocation line, not fleet-wide, because the unscoped version invented
  operands the existence oracle could not attest.
- fleet: fabrication count 130 to 124 tools on a full-PATH sweep, zero flags or
  subcommands lost, 2026-08-30

### S-005: docopt bracket-group flag row (LVM family)

- id: S-005
- looks like: |
        Check the consistency of volume group(s).
        vgck
      	[    --reportformat basic|json ]
      	[ COMMON_OPTIONS ]
      	[ VG|Tag ... ]
- tools: vgck, vgextend, vgrename, lvreduce, and the wider lv*/vg*/pv*
  family, lvextend
- handling: A whole physical line that is exactly one bracket group starting with a dash
  is a flag row, recognized by grammar::looks_like_bracket_flag_row, never
  folded into the general flag-start test because that would end lsof's own
  bracket continuation early. A bare tool-name line followed immediately by
  such a row also opens the usage block. Leading tabs now expand to the
  terminal tab stop so tab-indented rows measure deeper than a two-space
  heading; the same fix separately repaired a pre-existing defect in three
  squashfs-tools binaries and sotruss, where a deeply tab-indented description
  continuation starting with a dash was misread as a fabricated flag entry
  under raw character counting. A row whose alias run does not end at its
  first whitespace gap, ethtool's --all-groups | --groups form, is refused
  rather than merged. The family also writes one stanza per invocation form
  separated by a blank line; the usage block continues past the blank line
  into a later stanza only when the next stanza head repeats the tool's own
  name with a bare flag token or bracket-row evidence, recovering lvconvert's
  own 26 further stanzas that a blank line used to end the block on.
- fleet: 19 flags for vgck, 30 for vgextend, 21 for vgrename, about 1400 fleet-wide,
  2026-08-30

### S-006: abbreviation brackets glued onto a spelling

- id: S-006
- looks like: |
      OPTIONS := { -V[ersion] |
                    -s[tatistics] | -r[esolve] | -f[amily] inet|inet6|... }
- tools: ip
- handling: grammar::try_short/try_long recognize a bracket glued onto a run of one or
  more leading lowercase letters and read the whole word as one spelling, with
  abbrev recording how many leading characters the row displayed standalone. A
  value placeholder in upper or mixed case, or one opening with =, falls
  through untouched to ordinary value-spec grammar. Closes the old
  duplicate-short-flag collision between one-letter and multi-letter prefixes.
- fleet: not measured

### S-007: anchored value recovery on a run-mate row

- id: S-007
- looks like: |
      -h topic            show help
      -? topic            show help
      -help topic         show help
      --help topic        show help
- tools: ffplay, ffmpeg, ffprobe, byobu-disable, byobu-enable, bzmore,
  debconf-apt-progress, e4defrag, finalrd, iscsi_discovery, rust-gdbgui,
  unix_chkpwd, validlocale, xdg-user-dir
- handling: repair_single_dash_long_options rewrites -help's glued "elp" into the real
  spelling -help but clears its value, since the row's real value is already
  gone by then. recover_anchored_values restores it only when the raw document
  still contains "-help topic" as a literal phrase and a run-mate row already
  carries the value cleanly. It never folds the rows into one entity; a first
  version that did was reverted after false merges surfaced, see S-024.
- fleet: not measured

### S-008: single-dash long option collides with its double-dash twin

- id: S-008
- looks like: |
      -h, -help topic     show help
      --help topic        show help
- tools: ffplay, ffmpeg, ffprobe
- handling: Entity::long reported "-help" and "--help" as the same bare name "help", so
  merge_entity_bucket's short/long reconstruction kept only one dash count and
  silently dropped the other. The long-name key now carries dash count,
  L:1:help versus L:2:help, so the two spellings are never bucketed together
  unless a source genuinely repeats the identical spelling.
- fleet: not measured

### S-009: layout-driven option table, column detected per block

- id: S-009
- looks like: |
      -v, --verbose    Enable verbose output
      -q, --quiet      Suppress output
- tools: unnamed
- handling: Lines are grouped by leading-whitespace runs and indentation depth so a
  short flag, long flag and description tokenize structurally regardless of
  exact spacing. The description column is detected once per block, not once
  per row, and applied to the whole block.
- fleet: not measured

### S-010: section headings preserved as Flag::group

- id: S-010
- looks like: |
       Main operation mode:
        -A, --catenate, --concatenate   append tar files to an archive
        -c, --create               create a new archive
- tools: tar, git
- handling: A heading is recognized by relative indentation, any line whose next
  non-blank neighbour is indented further introduces that neighbour's block,
  since real headings sit at no fixed column. tar groups 171 flags this way;
  git groups commands by task.
- fleet: not measured

### S-011: hanging-indent prose misread as a heading

- id: S-011
- looks like: |
      When a filename is '-', nano reads data
        from standard input.
- tools: nano, update-xmlcatalog, dpkg, gcc, lto-dump, objdump, arptables,
  fail2ban-client, wpa_cli, zic, bpfcc
- handling: Three shapes share one remedy family. A sentence that ends on the promoted
  line, GNU argp's mandatory-arguments notice among them, is suppressed as a
  group label while its block parses unchanged; a colon-terminated heading
  that happens to read as a full sentence, such as gcc's, is left alone
  because it never ends in a full stop. A backslash-marked wrap,
  update-xmlcatalog's synopsis among 7 tools and 16 distinct lines, gets the
  same suppression. A sentence that continues onto the indented line is worse:
  dpkg's cross-reference sentence acquired both a fake section divider and a
  -f, --field option belonging to a different program, so the whole wrapped
  region is contained by a comma-terminated, multi-word, single-field line
  test and neither heading nor entries are recovered from it. A separate guard
  keeps an ordinary bug-report address sentence from opening this same
  containment fence on its own.
- fleet: 205 tools and 211 distinct tool-and-line pairs for the ended-sentence shape
  (56 inherit one GNU sentence, 13 another), 7 tools and 16 lines for the
  backslash shape, over 2301 frozen captures, 2026-08-30

### S-012: usage stanza labelled by its own description

- id: S-012
- looks like: |
        Start the lockspace of a shared VG in lvmlockd.
        vgchange --lockstart
      	[ -S|--select String ]
      	[ COMMON_OPTIONS ]
- tools: vgchange, and the wider lv*/vg*/pv* family
- handling: A multi-variant tool's own description sentence, not the invocation head
  line, becomes the flag group's label; the head line becomes a usage entry
  instead. The rule is anchored on the head being a recognized stanza head
  with exactly one bare flag token, and the sentence above it must be a lone,
  full-stop-terminated line of at least three words that is neither the tool's
  own name nor notation. With no such sentence the head-line label stands
  unchanged.
- fleet: not measured

### S-013: never invent subcommands from wrapped description lines

- id: S-013
- looks like: |
      -f, --format=FORMAT     archive format is FORMAT
                             FORMAT is one of the following:
                               gnu   GNU tar 1.13.x format
- tools: tar, dd, less, apt-get, git
- handling: A command block must be introduced by a recognized heading; layout alone is
  never sufficient evidence. A line at the description column with nothing at
  the name column is a continuation, never a new row. A candidate name must
  match a bare identifier shape, which is what refuses apt-get's own
  description paragraph (it previously fabricated the, information, about and
  them as subcommands). git's own command-group headings never say the word
  command themselves, so command mode is seeded from the leading blurb
  sentence that introduces them instead, never from the usage synopsis line's
  own COMMAND placeholder. tar previously gained 39 phantom subcommands, dd
  40, less 65, from wrapped continuation lines and enum values read as
  commands.
- fleet: not measured

### S-014: indented bare-word block as a flag's choices

- id: S-014
- looks like: |
        --format=FORMAT
            gnu
            oldgnu
            pax
            posix
- tools: tar, automake, cp
- handling: An indented list nested under a flag is that flag's choices, never
  subcommands. A per-value description, when the source documents one, is kept
  on the value rather than dropped, and a block with no plausible owning flag
  is dropped rather than guessed at. Ownership is proven only two ways, the
  heading names the flag's long spelling directly or the heading contains the
  flag's own value_name as a whole word (a one-character value name is
  excluded from the second proof, since no real placeholder is a single
  letter); automake's Warning categories heading and cp's VERSION_CONTROL
  block both name no flag literally and attach bare names only to the trailing
  flag as a base fallback, never a description. See S-025 for the still-open
  morphological-variant gap this leaves.
- fleet: not measured

### S-015: described choice values in a scope-flag sub-table

- id: S-015
- looks like: |
      -flags             <flags>      ED.VAS..... (default 0)
           unaligned                    .D.V....... allow decoders to produce unaligned output
           gray                         ED.V....... only decode/encode grayscale
- tools: ffmpeg, ffplay
- handling: A row strictly deeper-indented than a flag row directly above it, whose name
  column is a bare dashless word separated by a genuine aligned column gap,
  attaches to that flag's choices as one described value. The scope-flag and
  numeric value columns stay inside the description unparsed; mandible does
  not model what they mean. This nests directly under the flag's own row with
  no heading of any kind governing it, so it is recognized entirely inside the
  flags-block scanner's own continuation handling rather than through the
  heading-block matcher.
- fleet: not measured

### S-016: headingless invocation table naming the tool itself

- id: S-016
- looks like: |
          btrfs balance start [options] <path>
              Balance chunks across the devices
          btrfs device add [options] <device> [<device>...] <path>
- tools: btrfs
- handling: scan_headingless_invocation_table admits a run of rows only when there are
  at least two name-row and deeper-description-row pairs, every row starts
  with the tool's own name at a word boundary, every emitted name is checked
  to occur literally in the raw text, and only the leading run of name-shaped
  tokens after the tool's name contributes. Emission goes two levels deep.
  Every produced node is invocation_attested true, heading_attested false.
- fleet: not measured

### S-017: headed command table with a non-standard separator

- id: S-017
- looks like: |
        status [verbose] = get current WPA/EAPOL/EAP status
        ifname = get current interface name
- tools: wpa_cli, apt-ftparchive, fail2ban-client, trash-put, wpa_supplicant
- handling: scan_bare_command_table reads a row's name as only its leading name-shaped
  token, never a run. A " = " separator, found the same way as " - ", gives
  the description; its absence leaves the node honestly undescribed. Refused
  outright if any row in the block has a real column gap or a " - " separator,
  so it never competes with an already-working shape. It is gated on the
  current heading's own text mentioning command rather than an inherited
  sticky mode, since a wrapped description continuation under
  fail2ban-client's own real table once fabricated of, the, restarting and
  option as subcommands, and it requires distinct qualifying names rather than
  raw rows, since trash-put's own worked example repeats one program's name
  twice. Every recovered row is invocation_attested, never heading_attested,
  since these tables belong to daemon-control clients whose commands are
  runtime control verbs that a probe could act on rather than argv
  subcommands.
- fleet: not measured

### S-018: heading sharing its own physical line with the first row

- id: S-018
- looks like: |
      Commands: packages binarypath [overridefile [pathprefix]]
                sources srcpath [overridefile [pathprefix]]
- tools: apt-ftparchive
- handling: split_heading_inline_row recovers this special case, using
  is_section_heading_line to tell a real if unconventional heading label from
  an ordinary sentence merely ending in a colon. Without the split, this text
  became part of the heading string itself and the tool reported zero
  subcommands. The heading is split at its colon into a label and a trailing
  row inserted as the first command-table entry; rows are single-spaced, not
  column-aligned, so only a recognized heading and a name-shaped leading token
  confirm the trailing text is a real row.
- fleet: not measured

### S-019: pseudo-heading rewind inside a sticky command block

- id: S-019
- looks like: |
      Command:
        start   Starts Fail2ban server
      of the jails, restarting is not needed
- tools: fail2ban-client, trash-put
- handling: A wrapped description continuation, once command_mode is stuck on from an
  earlier real heading, can pass every row guard by shape alone. Gated on the
  current heading itself mentioning command(s) rather than on an inherited
  mode, and on the count of distinct qualifying names, not raw qualifying
  rows, since trash-put's worked example repeats one program name twice.
- fleet: not measured

### S-020: modifier table (letter glued to an operation)

- id: S-020
- looks like: |
      [d] - delete file(s) from the archive
      [r] - replace existing or insert new file(s) into the archive
      [v] - verbosely list files processed
- tools: ar, llvm-ar
- handling: Keyed on the row, never the heading, since ar's own modifier heading also
  contains the word command and would otherwise be sent down the subcommand
  path. A modifier row opens a bracket, closes it on the same line, holds
  exactly one ASCII letter, optionally an operand, then an explicit separator
  and a description. A digit or a bare single space is refused, which keeps
  pygettext3's numbered footnotes out.
- fleet: 2301 frozen captures checked, only pygettext3 near-miss, 2026-08-30

### S-021: argfile row (@<file>)

- id: S-021
- looks like: |
      @file              Read command-line options from file
- tools: ar, nm, objdump, readelf, size, addr2line, as, ld, ranlib, llvm-ar, jmod,
  jlink
- handling: Recognized by shape, an @ glued to a bracketed placeholder or an
  all-uppercase word at the row's own name column, never by tool name. Becomes
  the argfile sigil flag rather than being left unemitted, which is what
  happened before: the row safely ended a flags block but produced no entity
  at all.
- fleet: not measured

### S-022: "operations" heading recognized like commands

- id: S-022
- looks like: |
      OPERATIONS:
        d - delete [files] from the archive
        m[abi] - move [files] in the archive
- tools: llvm-ar, jmod, mount, ar, automake-1.16
- handling: Rule 1's heading vocabulary is extended to recognize a heading mentioning
  operation or operations as a whole word, the same footing as
  command(s)/subcommand(s), for heading recognition only. Does not seed the
  prose-triggered command_mode chain, since the word appears in ordinary
  description prose across 141 tools. Mount's own Operations: heading
  introduces an ordinary flags table, so the flags-block recognizer must claim
  it first before this heading test is ever consulted, the same order ar's own
  commands-worded modifier heading already relies on. Recovered operations are
  heading_attested true.
- fleet: 22 tools carry such a heading, 20 are ordinary flag tables, 2 are genuine
  operation tables, 2026-08-30

### S-023: environment variable section

- id: S-023
- looks like: |
      ENVIRONMENT:
          BPFTRACE_STRLEN     [default: 64]
              bpftrace string size
- tools: bpftrace, node, nodejs, sqfstar
- handling: Heading-keyed, never row-keyed, the opposite choice from the modifier table,
  because a bare identifier and description is not distinguishable from an
  ordinary settings table without the heading's own evidence. Recognized only
  when a heading reduces to exactly environment, environment variable, or
  environment variables. The row floor is one, not two, since the heading is
  already positive evidence.
- fleet: 57 tool directories, 2301 frozen captures, 2026-08-30

### S-024: alias-run fold has no safe discriminator

- id: S-024
- looks like: |
      -w    Ignored.
      -X    Ignored.
- tools: as, lto-dump, sysctl, mkfs.bfs, llvm-size-18
- handling: Adjacent single-spelling rows with identical description, value and at most
  one long-like name still fabricate merges. Repeat count of the shared
  description does not discriminate: lto-dump's "[disabled]" repeats 505 times
  in one document and several genuine aliases sit at the same repeat count as
  the fabrications. Withdrawn; only the narrower anchored value recovery
  (S-007) shipped. Needs an axis other than description equality.
- fleet: 505 rows in one document, 2026-08-31

### S-025: choices-block ownership by morphological variant

- id: S-025
- looks like: |
      -W, --warnings=CATEGORY
      Warning categories include:
- tools: automake, cp, install, ln, mv
- handling: A heading's choices attach only when the heading literally names the flag or
  contains its value_name as a whole word. "categories" does not equal
  CATEGORY, so automake's block and the cp/install/ln/mv backup-control block
  stay unattached and render as before. A stem or plural rule needs a fleet
  measurement of its false-positive cost before it can be admitted.
- fleet: not measured, 2026-08-31

### S-026: list row naming independent options, read as one alias

- id: S-026
- looks like: |
      -h|--help|-b|-S|-t|-T|-l
- tools: ld, socat-mux
- handling: A row's own grammar says alias list, so it reads as one multi-spelling
  entity, but the tool means any of these independent flags. No shape-level
  evidence distinguishes the two readings from an ordinary -h|--help pair.
  Accepted as a faithful reading of the row grammar; a fix needs evidence
  outside the row itself.
- fleet: not measured, 2026-08-31

### S-027: wrapped description starting with a dash-led word

- id: S-027
- looks like: |
      Use dpkg with -b, --build, -c, --contents,
      -e, --control, -x, --extract, -f, --field,
      ... on archives (type dpkg-deb --help).
- tools: zgrep, resolvconf, jpackage, dpkg
- handling: A dash-led line at its paragraph's own indent now reads as prose, not a
  flag row, when the previous physical line is non-blank, not sentence-final,
  and prose, and the candidate carries no description column. The rule
  cascades, so a comma list wrapping across several dash-led lines reads as
  one continuation. dpkg's hanging-indent wrap stays on the separate S-011
  remedy. The recovered paragraph is appended to the node description so the
  sentence still reaches the tree.
- fleet: detector fires on 21 tools, 26 flags, over a 2318-tool sweep,
  2026-09-03. Quotable as calibrated: zgrep and resolvconf now carry human
  verdicts, the detector fires on both, and it stays silent on all 39
  judged-correct tools that have a fixture.

### S-028: smaller unshipped defects

- id: S-028
- looks like: |
      ffplay --help (11k lines)
- tools: ffplay, ffmpeg, as
- handling: ffplay's --help parses in about 3.3s in debug builds, over the corpus
  runner's 100ms advisory ceiling, non-failing. The contract schema gap is
  closed: must_describe asserts a flag's description text, and
  corpus/jmod/17.0.20 uses it so that recovery is no longer guarded only by
  the snapshot. is_value_spec_token accepts an all-uppercase
  prose word as a value name, documented in tests, unobserved in the fleet.
  as-style =WORD placeholder-word choices are still read as choices, not
  alternative placeholders. Both remaining items need a fleet measurement of
  their false-positive cost before a rule can be admitted.
- fleet: not measured, 2026-08-31

### S-029: stderr-only help behind a decorator banner

- id: S-029
- looks like: |
      ------> --help <------
- tools: bzless (delegates to bzip2's own --help on stderr)
- handling: stdout carries only the decorator banner, read as one garbage flag with
  long="----". The real flag table lives on stderr and is not parsed at all;
  no flag currently has long=="help". Left broken on purpose.
- fleet: not measured

### S-030: flag and value glued on one physical line, two flags lost

- id: S-030
- looks like: |
      -u <username> -p <password>
- tools: pastebinit
- handling: The whole line becomes one entry, short='u', value_name='<username>',
  description='-p <password>'; -p is dropped as its own flag entirely,
  swallowed as literal description text on -u. Left broken.
- fleet: not measured

### S-031: capitalized description word read as a fabricated value

- id: S-031
- looks like: |
      -l List all supported pastebins
      --md5 Control MD5 generation
- tools: pastebinit, apt-ftparchive
- handling: find_sentence_start_gap recognizes a bare boolean flag followed directly by
  its description, so the description's own leading capitalized word, List,
  Print, Control, is read as prose rather than a fabricated required value.
  Originally written for apt-ftparchive's --md5 row and later found to already
  cover pastebinit's own rows.
- fleet: not measured

### S-032: whole usage block with no per-flag table at all

- id: S-032
- looks like: |
      pptpsetup --create tunnel_name --server address
          --username name --password password
          [--encrypt] [--start]
- tools: pptpsetup
- handling: The block spans multiple lines with no per-flag table, only a trailing
  bullet list of prose descriptions in invocation order with no flag names
  repeated. Only one of seven documented flags survives. Distinct from the
  bare-block-extent defect (S-033): here no usage block is ever opened at all.
  Left broken.
- fleet: not measured

### S-033: bare-word operand block runs through trailing flag rows

- id: S-033
- looks like: |
      where:
        bpt=BPT            block size
        --progress
        --verify
- tools: sg_dd
- handling: bare_block_end now ends a bare-word operand block before flag-shaped rows at
  the same indent, instead of running through them and turning six real flags
  into meaningless choices. The second synopsis paragraph after a blank line
  still is not read as usage at all, a separate, unfixed singleton.
- fleet: not measured

### S-034: two-character bundled short flag below the cluster floor

- id: S-034
- looks like: |
      -I certificate_identity -s ca_key [-hU] ...
- tools: ssh-keygen, umount, ssh-agent, lessecho, mandoc, psfxtable, rpcgen,
  sg_map, which, xxd
- handling: grammar::MIN_CLUSTER_MEMBERS is 3, so a two-character bundle like -hU is
  left alone: -U reads as a fabricated required value on -h. About half of the
  fleet's two-character population are genuine collapses like this one; the
  other half are correct multi-character single-dash flags such as rpcgen -Ss
  or xxd -ps, and nothing about shape alone separates the two halves. Left
  broken deliberately, the fix's documented lower bound.
- fleet: not measured

### S-035: repeated-letter and glued single-dash long option

- id: S-035
- looks like: |
      -vv    print more debugging output
      -help  show this help
- tools: killsnoop.bt, opensnoop.bt, naptime.bt, tcpaccept.bt,
  threadsnoop.bt, qemu-arm64-static, bpftrace, ntfsfallocate, lessecho,
  dbiprof, gcc, cpp, ffplay, ffprobe, rpcgen, Ghostscript, grub-file, icupkg,
  iscsistart, llvm-jitlink-18
- handling: repair_single_dash_long_options rewrites a short flag plus a glued run, like
  -h plus "elp", into the real single-dash spelling. A repeated-letter form
  like -vv becomes long=="vv", single_dash==true, no value, rather than
  short='v' with value_name='v'. The bare boolean the repair reads as
  evidence, -v, -d or -k, is asserted alongside it since the repair must not
  consume it. The rewrite requires an option-table source, a bare short flag
  with a required value, a name-shaped lowercase swallowed tail at least two
  characters long, and the reconstructed token occurring glued in the tool's
  own raw text; lessecho's genuine -nn flag is left alone because lessecho
  never writes a bare -n on its own row to serve as evidence.
- fleet: 11 rows recovered for qemu-arm64-static alone; fleet-wide 132 tools and 8784
  flags (17.6 percent of every flag extracted), underscore admission moves 17
  more tools and 604 flag spellings, 2254 tools swept on aarch64 Ubuntu 24.04;
  repeated-letter form separately measured at 6 of 94 seed-2 audit verdicts

### S-036: three flag-description pairs packed on one physical line

- id: S-036
- looks like: |
        -?|-h list help          -a AND selections (OR)     -b avoid kernel blocks
        -c c  cmd c ^c /c/[bix]  +c w  COMMAND width (9)    +d s  dir s files
- tools: lsof, unzip, infocmp, zipinfo, nano, arptables, patch, awk,
  debconf, thin_metadata_size
- handling: block_is_multi_column/fields_in_line detect a block's own column count from
  recurring flag-shaped cells at the same character offset across three or
  more rows, then split each row into its real per-flag pairs. Previously the
  generic parser detected only one description column and swallowed the rest
  of the line into the first flag's description at full confidence. A field
  that carries no real description text yet stays open across further
  flag-shaped cells, which keeps an alias pair like nano's short and long
  spelling together and protects arptables's lowercase value placeholder from
  being read as a second, fabricated flag.
- fleet: 5 flags recovered (-a -b -l -t -v), 2026-08-30

### S-037: usage synopsis wraps at the marker's own indent, not a hanging indent

- id: S-037
- looks like: |
       usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [+D D] [+|-E] [+|-e s] [+|-f[gG]]
       [-F [f]] [-g [s]] [-i [i]] [+|-L [l]] [+m [m]] [+|-M] [-o [o]] [-p s]
       [+|-r [t]] [-s [p:s]] [-S [t]] [-T [t]] [-u s] [+|-w] [-x [fl]] [--] [names]
- tools: lsof, du, expand, grub-set-default, lzless, sbverify, sha1sum
- handling: Every continuation line sits at the same column as the usage: marker itself,
  not further indented the way git's wrap is. Reading continuation purely by
  "more indented than the block's base indent" treated this as the block
  already having ended, silently dropping six flags documented only in these
  lines. du's own block instead ends with an ordinary prose sentence at that
  same column with no blank separator, so a line at or below the base indent
  continues the block only when it still reads as usage grammar, opening with
  a bracket, angle bracket or brace delimiter.
- fleet: 27 to 21 flags lost by the earlier defect, then recovered, 2026-08-30

### S-038: alias separator swallowed into the value placeholder

- id: S-038
- looks like: |
      -p PID, --pid PID  trace this PID only
      --count=OC|-c OC
- tools: filegone-bpfcc, gethostlatency-bpfcc, sg_sanitize
- handling: grammar::alias_continues resumes an alias run past the value spec, so an
  argparse-style "-p PID, --pid PID" row yields short, long and value_name
  correctly instead of leaking the comma into the placeholder and dropping
  --pid. sg3_utils's own "--count=OC|-c OC" form, long first, joined with =,
  is the same family's second arm and is fixed the same way, short='c',
  long='count', value_name='OC'.
- fleet: not measured

### S-039: getopt error line on stderr misread as description

- id: S-039
- looks like: |
      unknown option -- -
- tools: ssh-keygen, c_rehash, myisamlog, ping, nginx, sshd, and all fifteen
  probed xfs_* tools, arptables-nft-save, arptables-save, byobu-quiet,
  byobu-silent, cgi-fcgi, cpgr, cppw, debugfs, delv, dumpe2fs,
  ebtables-nft-save, ebtables-save, filan, ip6tables-legacy-save,
  ip6tables-nft-save, ip6tables-save, iptables-legacy-save, iptables-nft-save,
  iptables-save, lvmdump, mkfs.xfs, mytop, nfsconf, nslookup, nsupdate,
  ntfstruncate, pppoe-discovery, procan, prtstat, resize2fs, rsyslogd,
  socat-broker.sh, socat-chain.sh, socat1, tnftp, xfs_rtcp, zipdetails,
  debconf-copydb, mke2fs, ping4, ping6, sftp, slogin, ssh-copy-id, ssh-keyscan
- handling: is_option_error_line/is_option_error_paragraph recognize the tool's own
  getopt complaint about the --help probe and drop it rather than surface it
  as the tool's description. When it was the only leading paragraph, the
  description field is now honestly absent rather than replaced with something
  else. Every line in the paragraph must match one of four conventional
  complaint phrases or busybox's own fixed usage-error sentence, and the
  predicate is allowed to drop the tool's only leading paragraph since a
  complaint is never a real description regardless of what else is available.
- fleet: 116 tools changed, 52 tools measured and excluded, 2301 frozen captures,
  2026-08-22

### S-040: bare-token walk swallows a mandatory flag's own value

- id: S-040
- looks like: |
      ssh-keygen -D pkcs11
      ssh-keygen -M generate
- tools: ssh-keygen, ip6tables
- handling: extract_usage_flags's bare-token walk used to read a mandatory, unbracketed
  flag's own value word as an unrelated, silently-dropped bare token, so the
  flag looked like a boolean it is not. Fixed so -D, -F, -I, -M, -R, -r and -s
  all carry their real, documented value.
- fleet: not measured

### S-041: bracketed optional operand never becomes a positional

- id: S-041
- looks like: |
      Usage: /usr/bin/bashbug [--help] [--version] [bug-report-email-address]
      usage: lessecho [-ox] ... [-a] file ...
- tools: bashbug, lessecho, vim.basic
- handling: An operand named only inside a bracket group at the tail of a usage line is
  not extracted as a positional; the tree currently carries no positionals at
  all for these tools even though the operand is real and documented. Open
  defect.
- fleet: detector fires on 150 tools, 150 flags, over a 2318-tool sweep,
  2026-09-03, down from 194 before the single-line synopsis repair.
  Calibrated and passing: 3 of 3 labelled members detected, 0 false alarms
  on seed 4.

### S-042: BNF production heading sharing its own row (iproute2 family)

- id: S-042
- looks like: |
      where  OBJECT := { address | addrlabel | amt | fou | help | ila | ioam | l2tp |
             OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |
- tools: ip, vdpa, bridge, dcb, devlink, rdma, delv, mariadb-admin,
  swift-recon-cron, whiptail
- handling: A column-0 line carrying a bare colon-equals is recognized as a grammar
  production rather than description prose. The iproute2 family also glues its
  heading to the first row with this same operator instead of a column gap, so
  the split rule accepts the operator as an alternative to a column gap once
  the remainder still looks like a flag. A bracket-only rule with no operator
  requirement would match 36 tools, most of them false positives such as
  pkgdata's parenthetical aside.
- fleet: 6 tools regain their first OPTIONS row, a 7th (ss) recovers nothing since
  its productions open on bare words, not dated

### S-043: BNF alternation row grammar (iproute2 OPTIONS block)

- id: S-043
- looks like: |
      OPTIONS := [ -V | --Version | -i | --iec | -j | --json
                 | -N | --Numeric | -p | --pretty
                 | -s | --statistics | -v | --verbose]
- tools: ip, vdpa, bridge, rdma, devlink, dcb
- handling: One physical row can list several distinct flags separated by pipes, and a
  wrapped continuation line can open on the pipe operator itself rather than
  repeating a spelling. Every segment must parse cleanly alone or the whole
  row is refused. An enclosing group's own closing bracket can land glued onto
  the last alternative and is stripped only when no matching opener exists
  earlier in the segment. All of this is gated on the block's heading already
  being a recognized BNF production, since a plain pipe is an ordinary alias
  separator elsewhere.
- fleet: 8 tools outside the iproute2 family regressed before this gate existed
  (btrfsck, dpkg, mkfs.btrfs, pvchange, sg_get_config, sg_write_x,
  update-java-alternatives, vgchange)

### S-044: positional row opens an options table

- id: S-044
- looks like: |
       <pid> [...]            send signal to every <pid> listed
       -q, --queue <value>    integer value to be sent with the signal
- tools: kill
- handling: A flags block may open with a positional row instead of a flag row. Deciding
  flags versus bare words from the first row alone sent the whole block down
  the wrong path and kill reported zero flags. The parser now recovers six
  fully described flags from the same document.
- fleet: not measured

### S-045: mixed-depth flags block with a dash-led continuation

- id: S-045
- looks like: |
      no verbatim excerpt in source
- tools: tar
- handling: Real help output can mix two entry depths in one block, a short and long
  flag at one column and a long-only flag deeper. The parser tracks the
  shallowest entry seen so far rather than a single shared indent floor. A
  deeply indented dash-led continuation line, such as tar's own wrapped
  --occurrence description, still counts as a continuation rather than a new
  entry.
- fleet: not measured

### S-046: leading non-flag rows budget before the first flag

- id: S-046
- looks like: |
      no verbatim excerpt in source
- tools: ar
- handling: A flags block may open with a few rows that are neither flags nor bare
  command names, such as ar's own bracketed modifier tokens. A bounded number
  of such rows may be skipped before the first real flag row, deliberately
  capped so looking for flags never becomes guessing.
- fleet: not measured

### S-047: packed flag rows with operand tokens and no description

- id: S-047
- looks like: |
      -amin N -anewer FILE -atime N -cmin N -cnewer FILE -context CONTEXT
      -exec COMMAND ; -exec COMMAND {} + -ok COMMAND ;
- tools: find
- handling: Several bare flag entries with operand tokens but no descriptions appear
  packed on one physical line, GNU find's Tests, Actions and Normal options
  tables being the clearest case. The whole block must split cleanly this way
  and at least one line must carry two or more entries before any line is
  treated as this shape. A token that is neither a new entry opener nor a
  recognized operand refuses the whole line rather than guessing a boundary.
- fleet: not measured

### S-048: value placeholder wraps mid-word onto the next line

- id: S-048
- looks like: |
      --target-platform <String: target-
        platform>
- tools: jmod, msgcat, msgcomm
- handling: A flag's value placeholder can wrap mid-word onto the next physical line.
  The parser joins such a continuation into the value name rather than the
  description, detected by an unclosed angle bracket that opens only at a
  token boundary. msgcat's own short flag spelled with a literal angle bracket
  glued onto the dash is not mistaken for an open placeholder.
- fleet: not measured

### S-049: equals-prefixed enumerated value sub-row

- id: S-049
- looks like: |
      =default            -   default
      =gnu            -   gnu
- tools: llvm-ar
- handling: A continuation line opening with an equals sign followed by a bare word
  names one of the flag's possible values rather than more description text.
  These route into the flag's choices list with no description, since llvm-ar
  never documents a per-value explanation on this shape.
- fleet: not measured, llvm-ar is the only tool observed opening a continuation line
  this way

### S-050: deeper-indented command table follows a flags block

- id: S-050
- looks like: |
        --version         print version string

          btrfs balance start [options] <path>
              Balance chunks across the devices
- tools: btrfs
- handling: A flags block can be followed by a deeper-indented command table whose rows
  are not that block's continuation. Indentation alone cannot distinguish this
  from an ordinary wrapped description, so at least two repeating
  name-and-description rows are required before a deeper run is treated as a
  separate table.
- fleet: not measured

### S-051: flag row ends in a colon, value list is its whole description

- id: S-051
- looks like: |
      --strip=[none|crc|unsafe|unused]:
          none (default): Retain all chunks.
          crc: Remove chunks with a bad CRC.
- tools: pngfix, pod2man
- handling: A flag row ending in a colon with nothing else means everything one indent
  deeper is that flag's only description, often a value-choice or keyword
  list. The nested-command-table detector would otherwise misread this wrapped
  explanation as a separate table and delete the whole description; that break
  is refused whenever the entry row itself carries no description of its own.
  Neither name starts with the tool's own name, so the headingless invocation
  table recognizer never reaches them either.
- fleet: not measured

### S-052: headingless flags block

- id: S-052
- looks like: |
      no verbatim excerpt in source
- tools: sed
- handling: A tool's help output can open its flags directly with no Options or Flags
  heading line at all; the current line already looks like a flag entry, so
  the block is scanned in place. Recovery is refused when something
  heading-shaped sits directly above the first flag row, which keeps a worked
  example's own Input:/Output: labels from being reopened as a real flags
  section.
- fleet: not measured

### S-053: dash-separated bare-word command entry

- id: S-053
- looks like: |
      update - Retrieve new lists of packages
- tools: apt-get, aarch64-linux-gnu-ar, aarch64-linux-gnu-gcc-ar
- handling: A bare-word command table can separate name from description with a
  space-dash-space run instead of a column gap. The ordinary column gap is
  tried first and this fallback only applies when no column gap exists, so a
  tool already using column alignment is unaffected. A name's own internal
  hyphens never match, since they lack a space on at least one side.
- fleet: not measured

### S-054: synonym column instead of a description

- id: S-054
- looks like: |
      no verbatim excerpt in source
- tools: awk
- handling: Some tools lay out a second column naming an equivalent flag spelling rather
  than a description, such as awk's POSIX short options beside their GNU long
  equivalents. A lone dash-led token with no other words is treated as a
  synonym, not a description, so the parser reports no description rather than
  fabricating one.
- fleet: not measured, would otherwise have reported 28 flags fully described before
  the guard

### S-055: tab-separated column gap

- id: S-055
- looks like: |
      --list-enrolled				List the enrolled keys
- tools: mokutil
- handling: A table can separate its columns with a single tab rather than two or more
  spaces. Any run containing a tab is treated as a column gap on its own,
  since a tab already advances further than the minimum space run required.
  mokutil reported 38 flags with zero described before this rule.
- fleet: not measured fleet-wide, mokutil measured at 38 flags, 0 described, before
  the fix

### S-056: single space after an unpadded long flag falls back to a boundary split

- id: S-056
- looks like: |
         --abstract-unix-socket <path> Connect via abstract Unix domain socket
      -a, --append      Append to target file when uploading
- tools: curl
- handling: Some tools right-pad short specs to a fixed width but run only a single
  space after a long one. The parser splits right after the closing bracket or
  angle bracket of a value placeholder when exactly one space and further
  content follow it.
- fleet: not measured fleet-wide, curl --help all measured at roughly 25 percent
  described before the fix

### S-057: equals sign as the description separator

- id: S-057
- looks like: |
      --verbose = be verbose
      -b = optional bridge interface name
      --root  = the root XML catalog
- tools: update-xmlcatalog, wpa_supplicant
- handling: A flag row can separate its spec from its description with a lone equals
  sign standing alone between whitespace, with no aligned column anywhere.
  Every token before the separator must be spec-shaped, then the leading
  equals and its following space are stripped from the recovered description.
- fleet: not measured fleet-wide, wpa_supplicant's roughly 28 flags measured at about
  9 percent parsed before the fix

### S-058: dash-token separator after a glued equals value

- id: S-058
- looks like: |
      --target=BFDNAME - specify the target object format as BFDNAME
- tools: ar, llc-18, opt-18, bugpoint-18
- handling: A value can be glued onto the flag with an equals sign, leaving no bracket
  and a lowercase description a sentence-start test would not catch. The
  parser finds an isolated dash token standing alone between whitespace and
  requires every preceding token to be spec-shaped, so a bare lowercase prose
  word after the spec never opens this fallback.
- fleet: not measured

### S-059: bracketed operand inside a dash-led bare-word row

- id: S-059
- looks like: |
      d - delete [files] from the archive
- tools: llvm-ar
- handling: An operand placeholder inside a dash-separated bare-word row can be mistaken
  for a flag row's own trailing value boundary. This boundary rule requires
  the line to start with a dash before applying, so an operations table row
  naming a bracketed operand is not split apart at the bracket and dropped.
  Cost llvm-ar-18 six operation entries before the guard existed.
- fleet: not measured

### S-060: usage-synopsis word standing in for the option list

- id: S-060
- looks like: |
      no verbatim excerpt in source
- tools: tar, pkgconf, dpkg-statoverride, vim, git
- handling: A usage synopsis can name its own option list with a word such as options or
  arguments rather than naming a real operand. A short fixed list of such
  words is excluded from becoming a fabricated positional. The list
  deliberately omits args and arg, since git and other tools use those words
  as genuine forwarded operands.
- fleet: not measured, vim anchor case confirmed with the maintainer, 2026-08-13

### S-061: continuation line too deep for a new flag entry

- id: S-061
- looks like: |
        -X, --gxditview[=RESOLUTION]   use groff and display through gxditview
                                   (X11):
                                   -X = -TX75, -X100 = -TX100
- tools: man
- handling: A continuation line can sit far past the tolerance for a new flag entry and
  is read as an ordinary continuation, joined verbatim into the description
  above it, even though its own text would otherwise qualify for a
  separator-based split. It is never offered to any gap-finder, since the
  block-level continuation rule already claimed it first.
- fleet: not measured

### S-062: heading shares its physical line with the first flag row

- id: S-062
- looks like: |
      Options:  -h, --help                    print this message
                -V, --version                 print the program version
- tools: uconv, zipinfo, delv, mariadb-admin, swift-recon-cron, whiptail
- handling: A section heading and the first row of its table can share one physical
  line. Splitting recognizes a real column gap after the label with the
  remainder looking like a flag, replacing the earlier behaviour of promoting
  the whole line to a heading and losing the first flag under any spelling.
- fleet: 2 tools lose exactly this row over 2301 frozen captures; a heading followed
  by any column gap matches 12 tools, the other 10 being second heading
  columns or wrapped prose

### S-063: dash-underline column header row

- id: S-063
- looks like: |
      ------  -----------
- tools: jmod
- handling: A table can carry its own decorative column-underline row made of nothing
  but dashes and whitespace. Each whitespace-delimited run is recognized as
  its own dash-underline token so the row never carries literal dashes into a
  flag's group label.
- fleet: not measured

### S-064: decorative dash-bracketed divider heading

- id: S-064
- looks like: |
      ------- Listing options -------
- tools: tree
- handling: A section divider can be written as a dash run, a plain-word label and
  another dash run, with no trailing colon. It is dropped honestly rather than
  turned into a real group label, and the same shape is also refused as a
  genuine usage continuation. tree itself carries seven such headings.
- fleet: not measured

### S-065: same-indent word grid of bare command names

- id: S-065
- looks like: |
      Standard commands
      asn1parse       ca              ciphers         cmp
      cms             crl             crl2pkcs7       dgst
- tools: openssl
- handling: Some tools list their subcommands as a same-indent grid of bare names with
  no descriptions, openssl writing this to stderr with no Usage line or
  indentation. Starting a grid requires at least three aligned columns
  separated by a run of two or more spaces, since a wrapped prose paragraph
  separates its words with exactly one space. apt-get's own description
  paragraph was read as seven fabricated subcommands before this rule.
- fleet: not measured

### S-066: rendered man page instead of ordinary help output

- id: S-066
- looks like: |
      GIT-BISECT(1)    Git Manual    GIT-BISECT(1)
- tools: git, byobu, byobu-screen, byobu-tmux, git-receive-pack, git-upload-archive,
  git-upload-pack
- handling: A rendered man page carries the identical name-and-section title at both
  margins of its first line with a centred title between them, recognized as a
  property of the roff output format rather than any tool. On a subcommand
  path the parser falls back to the short -h flag instead, recovering an
  ordinary option table for most of git's own subcommands this way. The same
  behaviour at a tool's own root is deliberately left alone, and man page
  prose read as commands once fabricated git bisect's own follows, testing,
  command and skipped subcommands.
- fleet: 6 named root-level binaries checked, not otherwise dated

### S-067: generic Options/Flags heading suppressed as a duplicate label

- id: S-067
- looks like: |
      no verbatim excerpt in source
- tools: gh, tar
- handling: A heading that only says Options or Flags subdivides nothing and would
  otherwise render the same label twice in a row. A short fixed list of such
  generic labels is suppressed from becoming a flag's group, while a genuinely
  descriptive heading such as tar's Main operation mode is kept, since it
  subdivides a 171-flag list into something scannable.
- fleet: not measured

### S-068: clap-style leading name/author/homepage banner

- id: S-068
- looks like: |
      zoxide 0.9.9
      Ajeet D'Souza <98ajeet@gmail.com>
      https://github.com/ajeetdsouza/zoxide

      A smarter cd command for your terminal
- tools: zoxide, cargo
- handling: Clap's own help template renders a name-and-version line, an author line and
  a homepage line as one leading paragraph before a blank line and the real
  description. Concatenating every leading column-zero line put the email
  address and URL into the shown description. The banner is recognized
  structurally, by an exact two-token name-version first line or a URL or
  email address anywhere in the paragraph, never by comparing against the
  tool's own name, and is only dropped when a later paragraph exists to fall
  back to.
- fleet: not measured

### S-069: verbatim example line kept on its own line inside a description

- id: S-069
- looks like: |
      Example: grep -i 'hello world' menu.h main.c
- tools: grep
- handling: The description is handed to the sanitizer with its line and paragraph
  structure intact rather than pre-flattened with spaces, since deciding which
  line break is a hard wrap and which is real structure belongs to the
  sanitizer, not the parser. Joining early would throw that evidence away
  first.
- fleet: not measured

### S-070: command-mode seeded from prose naming a VERSION-shaped placeholder

- id: S-070
- looks like: |
      These are common Git commands used in various situations:
        start a working area (see also: git help tutorial)
           clone      Clone a repository into a new directory
- tools: git, containerd, ctr, systemd-creds
- handling: Git's own command-group headings never say the word command, but the leading
  blurb introducing them does, so command mode is seeded only from that
  leading description, never from the usage synopsis line. Seeding from the
  synopsis was tried and reverted, because the ordinary docopt placeholder
  COMMAND there reached unrelated headings: containerd and ctr both write that
  placeholder and seeding from it alone turned their unrelated VERSION heading
  into a fabricated subcommand literally named their own version string.
- fleet: not measured

### S-071: Examples:/Report bugs region fenced off from real structure

- id: S-071
- looks like: |
      Examples:
        jar --update --file foo.jar --main-class com.foo.Main --module-version 1.0
            -C foo/ module-info.class
- tools: tar, bpftrace, jar
- handling: An Examples: or Report bugs heading is tracked as an ignorable region so its
  worked-example rows never become fake subcommands or flags. tar's own block
  contains lines starting with the bare word tar, which would otherwise look
  like a subcommand entry, and bpftrace's and jar's own example invocation
  lines are structurally indistinguishable from a genuine stanza head or flag
  row. A prose sentence can obscure the Examples: marker by sitting directly
  above it; the region reopens only on a dedent or an independently attested
  flag section at or below the marker's indent.
- fleet: not measured

### S-072: free-running REPL banner explosion

- id: S-072
- looks like: |
      Available commands are:
         l            - List all installed modules
         q            - Quit the program
- tools: instmodsh
- handling: A Perl REPL that ignores --help free-runs printing its own banner until a
  wall-clock cap kills the probe. The captured output repeated almost exactly
  and parsed into 58,663 duplicate-named subcommands. A hard cap of 4096
  recovered entries per probe, plus deduplication by name, bounds both the
  entry count and the downstream merge cost.
- fleet: 58663 duplicate subcommands from one specimen before the cap, not dated

### S-073: argparse subparser choice-braced pseudo-entry

- id: S-073
- looks like: |
      positional arguments:
        {init,build,run}
          init            Initialize a new widget
          build           Build the widget
- tools: unnamed (argparse add_subparsers() convention generally, smokecli test
  fixture)
- handling: Argparse's own subparser blocks get first refusal on a positional-arguments
  heading, keyed on the stronger structural evidence of a choice-braced
  pseudo-entry with deeper lines beneath it. An earlier version gated on the
  heading text alone reading positional arguments, which collapsed a
  twelve-level command tree fixture to a single node; the text check was
  dropped entirely in favor of the structural one.
- fleet: not measured

### S-074: flag rows run directly into the usage line with no heading

- id: S-074
- looks like: |
         curl [options...] <url>
- tools: curl
- handling: Curl indents its thirteen flag rows by one space directly under its usage
  line, with no blank separator and no Options heading. A usage continuation
  line is only ever an alternative invocation form and never opens with a
  dash, so a continuation line that reads as a flag entry ends the usage block
  instead of being absorbed into it.
- fleet: not measured

### S-075: heading indented underneath its own synopsis

- id: S-075
- looks like: |
      Usage: ar [emulation options] [-]{dmpqrstx}[abcCDfilMNoOPsSTuvV...] archive [member-file...]
       commands:
- tools: ar
- handling: Binutils ar indents its whole body, including a heading, under the synopsis
  line. Indentation alone reads every line after the first heading as a
  continuation, which used to join the heading and all eight command rows into
  one usage string and produce zero subcommands. A section heading always ends
  the usage block regardless of how far it is indented.
- fleet: not measured

### S-076: flush-left prose paragraph documents an option by name elsewhere

- id: S-076
- looks like: |
      options:
              --for-removal
        -l    --list

      The --for-removal option limits scanning or listing to APIs that are
      deprecated for removal.
- tools: jdeprscan, jdeps
- handling: The options table is a bare list of spellings with no description column,
  and every option's prose lives in its own flush-left paragraph further down
  the document. A qualifying paragraph must sit at a shallow indent with no
  line starting with a dash, and open with an article, one option spelling, an
  optional parenthesized alias list, then the word option, flag or switch. The
  backfill never creates a flag the table did not already list and never
  overwrites a description the table itself supplied.
- fleet: 8 flags moved from 0.0 percent with text to fully described

### S-077: negatable boolean flag with a bracketed no- prefix

- id: S-077
- looks like: |
        -S, --[no-]staged     restore the index
        --[no-]ignore-unmerged
                              ignore unmerged entries
- tools: git
- handling: GNU getopt_long's negatable-boolean convention writes a bracketed no- prefix
  inside the long option's own name. The long-option grammar required an
  alphanumeric immediately after the double dash, so a short-spelled negatable
  row silently lost its long name and a long-only negatable row was discarded
  entirely. The fix recovers the base name with a negatable flag set, and the
  long name never contains a bracket character afterward.
- fleet: not measured

### S-078: declared positional block under an argparse heading

- id: S-078
- looks like: |
      usage: uobjnew [-h] [-l {c,java,ruby,tcl}] [-v] pid [interval]

      positional arguments:
        pid                   process id to attach to
        interval              print every specified number of seconds
- tools: argparse-family tools (uobjnew)
- handling: Argparse's own positional arguments heading lists plain positional operands
  separately from the usage synopsis, which states only whether each is
  optional or repeatable. The block and the synopsis are merged rather than
  appended, so an operand named in both places gains a description without
  becoming two entries. This recognizer runs only once the subparser scan
  (S-073) has already declined.
- fleet: not measured

### S-079: self-similar subcommand probe result

- id: S-079
- looks like: |
      no verbatim excerpt in source
- tools: systemctl, llvm-ar, pnpm
- handling: Some tools permute --help to the front of their own argument processing
  regardless of what precedes it, so a probe comes back identical to help already
  returned for an ancestor command path. Reading that as the node's own children
  turns a harmless command list into an unbounded recursive re-probe. Whenever
  selected help repeats a strict path ancestor for the same resolved binary and
  current root generation, that node degrades to verbatim with an empty,
  known-complete child list instead of cascading further. Siblings, unrelated
  paths, other binaries and merely similar documents stay independent. The
  history keeps a hash and a length for each path, holds at most 4,096 documents,
  and records nothing once full, in which case the node parses normally.
- fleet: one live incident starved the UI thread for over 45 seconds on a 4-core
  machine; pnpm 11.22.0 repeats its `audit` help below the root, reproduced
  2026-09-03, not otherwise fleet-measured

### S-080: truncation confession (tool names its own missing content)

- id: S-080
- looks like: |
      This is not the full help, this menu is stripped into categories.
      Use "--help category" to get an overview of all categories.
      -h      -- print basic options
      -h long -- print more options
      --help={common|optimizers|params|target|warnings|...}
- tools: curl, ffmpeg, gcc
- handling: Some tools announce their own --help is partial and name the flag that gets
  the rest, in a quoted directive, an unquoted flag-table row, or a flag-value
  row opening a class enumeration. Each shape is matched on content only,
  never on tool name, and each recovered word is checked against a closed
  vocabulary before the parser probes it as a follow-up argv. curl's own all
  category is followed and recovers 258 flags against 12 from the plain
  --help; gcc's class word is recorded but deliberately not followed.
- fleet: curl --help recovers 12 flags, curl --help all recovers 258, not otherwise
  measured fleet-wide

### S-081: multi-byte glyph lands mid-character at a byte offset

- id: S-081
- looks like: |
      no verbatim excerpt in source
- tools: unnamed
- handling: A function that measures how much text a caller consumed does not assume two
  string parameters are related by simple truncation, and returns no separator
  found rather than panicking when a byte count lands off a character
  boundary. This exists because a box-drawing glyph in a real captured
  document once landed mid-character and shipped a crash; the fix degrades
  safely instead of trusting the call site.
- fleet: not measured, referenced as a shipped crash with no count given

### S-082: aligned second-spelling column read as description

- id: S-082
- looks like: |
       -A             --smarthome             Enable smart home key
        -l    --list
       -f progfile    --file=progfile
- tools: nano, jdeprscan, awk, gawk, nawk, ntfsmove, ntfswipe, less
- handling: A row's second column is itself another spelling of the same option, not the
  start of its description, but the ordinary description-gap finder cuts at
  the first wide gap and reads the long spelling as prose. jdeprscan's long
  spellings vanished from the tree entirely and every one of nano's 52 flags
  kept only its short spelling with the long one glued onto its description. A
  run qualifies only when exactly one of its leading cells is a long spelling,
  since a run of two shorts or two longs is a genuine two-column table of
  separate options.
- fleet: 24 adjacent cell pairs across 5 tools, 2301 frozen captures, 2026-08-22

### S-083: alias run keeps every recognized spelling, not just the first pair

- id: S-083
- looks like: |
      --replace -R chain rulenum
      -? -h --help
      --quiet --noquiet --verbose --noverbose
- tools: iptables, jdeprscan, dpkg-split, gold, screen, xxd, socat-mux.sh, pod2html
- handling: The parser keeps every recognized spelling in one run rather than only the
  first short and first long spelling found. A run may continue on bare
  whitespace only while the previous spelling was a genuine short flag; a
  further long-like spelling needs an explicit comma or pipe separator. A row
  that repeats its value placeholder after every spelling records the value
  once, on the first sighting, so the alias list can resume after it. Leftover
  text is never read as the next flag's own name serving as this flag's value.
- fleet: not measured

### S-084: delimited flag alternation inside an option-table row

- id: S-084
- looks like: |
      {-i|--input} <input xml file>
      {-v | --version}
- tools: cache_restore, eqn, xfs_io
- handling: A brace or bracket delimited group whose every member is a bare flag
  spelling is read as an alternation of flag spellings and rewritten into the
  ordinary comma-free alias list the rest of the grammar reads. Before this
  rule cache_restore lost all eight of its flags because a leading brace was
  never recognized as starting a flag entry, eqn read --version as carrying a
  literal closing-brace value, and xfs_io's row was never recognized as
  starting with a dash at all. A member carrying its own value inside the
  alternation is refused rather than guessed, since attributing the value to
  the right alternative is genuinely ambiguous.
- fleet: not measured

### S-085: duplicate spelling refused within one alias list

- id: S-085
- looks like: |
      -c, --check, --check=diagnose-first
      -? -h --help -help
- tools: sort, jdb
- handling: A spelling identical to one already collected in the same run is never
  recorded a second time. GNU sort restates the same name a second time as a
  value-bearing form, and jdb's own multi-letter single-dash spelling
  currently truncates to its first character and can collide with an
  already-collected short spelling. The guard leaves the rest of the fragment
  unconsumed rather than recording a repeated name.
- fleet: measured at zero legitimate occurrences fleet-wide before this grammar
  existed, not otherwise dated

### S-086: plus-or-minus flag notation, unmodeled

- id: S-086
- looks like: |
      +|-e s  exempt s *RISKY*
- tools: lsof
- handling: This third notation, meaning plus or minus e, is not modeled at all. The
  separator predicate requires a finished value placeholder on its left before
  treating a pipe as an alias separator, and a bare plus is not one, so the
  token is left alone rather than fabricating a short flag carrying a literal
  plus as its value. Recovering the real -e flag here is left as future work.
- fleet: not measured

### S-087: GNU getopt bundled cluster below the collapse floor, and above it

- id: S-087
- looks like: |
      [-2CDlNuVv]
      [-AbdDefhHIJKlLnNOpqStuUvxX#]
- tools: tmux, groff, tcpdump, od, pod2text, showmount, e2image, whereis,
  tree, e2fsck, tic, mkfs.ext4, badblocks, zipinfo, fc-validate, psfxtable,
  setfont, sg_map, umount.nfs, blkmapd, debconf-set-selections, dhcpcd,
  dumpe2fs, filefrag, grops, logsave, rpcbind, sm-notify, ssh, strace,
  vm-support, wpa_supplicant, xfs_copy, xfs_io
- handling: A usage synopsis token like this names a set of bundled boolean switches,
  not one flag with a long glued-on value, but the ordinary flag grammar alone
  cannot tell the two apart. A synopsis-only check splits the cluster into one
  boolean flag per member when it has at least three members, all distinct
  characters, and either the letters read in alphabetical order or the
  swallowed half mixes upper and lower case. The identical glued shape in an
  option-table row is never split, since that is a genuinely correct
  single-dash value convention such as gcc's -Wall. See S-034 for the
  deliberately unsplit two-character case below this floor.
- fleet: 58 tools and 465 destroyed flags on a full-PATH sweep of 2302 tools, average
  8 lost flags per affected tool, 22 at the worst, not dated

### S-088: parenthesized alternation group spanning many physical lines (LVM)

- id: S-088
- looks like: |
      vgchange
      ( -l|--logicalvolume Number,
        -p|--maxphysicalvolumes Number,
        -u|--uuid,
             --setautoactivation y|n )
- tools: vgchange, lvconvert, vgck
- handling: LVM's own convention states that for options listed in parentheses, any one
  of them is required. A bare open parenthesis immediately followed by a flag
  token opens the group, tracked by running paren depth across physical lines,
  including across a blank line, rather than per-line content, closing on the
  row carrying the matching close paren. Each member row's own leading dash,
  comma and closing parenthesis are stripped before the remainder is handed to
  the ordinary flag-spec grammar.
- fleet: not measured

### S-089: stanza head names exactly one mode-selecting flag

- id: S-089
- looks like: |
      Activate or deactivate LVs.
      vgchange -a|--activate y|n|ay
        [ -K|--ignoreactivationskip ]
- tools: vgchange, blkid, jar
- handling: A synopsis head line that repeats the tool's own name followed by exactly
  one flag spelling, naming no second flag anywhere else on the line,
  documents that mode's own selecting flag. Refused when a second flag appears
  anywhere in the line, since blkid glues a second flag inside a bracket group
  and jar's example line chains several real flags with no brackets, either of
  which would otherwise fabricate one merged flag or drop a real one.
- fleet: not measured

### S-090: table border decoration between a heading and its rows

- id: S-090
- looks like: |
      ------  -----------
- tools: jmod, apparmor_parser
- handling: A token made of nothing but three or more dashes sitting directly under a
  two-column heading is table border decoration, refused as a flag row so it
  cannot fabricate a flag literally named with dashes. A run of exactly two
  dashes stays eligible, since a bare double-dash end-of-options marker is a
  real, meaningful row in some conventions.
- fleet: not measured

### S-091: help text split across stdout and stderr, judged by shape

- id: S-091
- looks like: |
      cmp_main:...:CMP info: using section(s) 'cmp' of OpenSSL configuration file ...
      Usage: cmp [options]
      Valid options are:
- tools: openssl (roughly 150 subcommands in this shape, cmp among them), mkfs.fat,
  tune2fs, btrfs-convert, xfs_scrub, encguess, ntfssecaudit
- handling: Some tools print diagnostic banner lines to one stream and their entire real
  help document to the other, so testing only whether a stream is non-empty
  picks the wrong one. Each stream is judged on its own structural
  plausibility, preferring the one that actually looks like help output when
  the two disagree, defaulting to stdout on every other combination including
  a tie. The raw verbatim pane keeps both streams separate and labelled rather
  than merging them.
- fleet: 200 of 656 fleet-wide fabrications attributed to a second copy of this rule
  drifting out of sync, not dated

### S-092: flush-left settings table misread as subcommands

- id: S-092
- looks like: |
      Variables (--variable-name=value)
      and boolean options {FALSE|TRUE}      Value (after reading options)
      commit                                0
      init-command                          (No default value)
- tools: mysqlslap, dnf
- handling: At a shared flush-left indent nothing structurally separates a heading from
  an ordinary data row, so every row becomes a candidate heading for the rows
  beneath it, and a row like init-command satisfies a naive commands-word
  test. A dedicated check now requires the heading to be a genuinely
  recognized command heading and every row to be column-aligned, keeping a
  flush-left table of settings from ever being promoted to subcommands. A row
  that is itself column-aligned like a table row is also refused as a
  candidate heading.
- fleet: 28 fabricated subcommands from one specimen before the fix, not dated

### S-093: flat comma-separated applet list (busybox)

- id: S-093
- looks like: |
      Currently defined functions:
          [, [[, acpid, add-shell, addgroup, adduser, adjtimex, ar, arch, arp,
          arping, ash, awk, base32, base64, basename, bc, bunzip2, bzcat, ...
- tools: busybox
- handling: Busybox lists every applet as one flat, tab-indented, comma-separated run
  with no per-entry description and no column gap anywhere, a shape the
  ordinary one-entry-per-line bare-block scan cannot express. A dedicated scan
  is gated on a profile flag rather than loosening the shared engine's general
  grid rule for every framework, and on the heading already being recognized
  or continuing a command-mode chain, since the heading text itself, currently
  defined functions, does not contain the word command.
- fleet: recovers over 250 real applets on this machine's busybox, not dated

### S-094: non-command "help topics" heading breaks a sticky command chain

- id: S-094
- looks like: |
      no verbatim excerpt in source
- tools: gh
- handling: Cobra applications commonly add a help topics section listing names, such as
  environment or reference, that are documentation topics rather than
  invokable subcommands. Because this section sits at the same indent as, and
  right after, several real command groups, the engine's own same-indent
  sticky chain rule would otherwise carry command mode straight through it. A
  framework-specific non-command heading marker stops the section itself from
  being read as commands and breaks the sticky chain for anything that
  follows.
- fleet: not measured

### S-095: plus-prefixed option row

- id: S-095
- looks like: |
      +			Start at end of file
      +<lnum>		Start at line <lnum>
- tools: vim.basic, vim, vi, view, rvim, ex
- handling: An option row whose name begins with a plus is not recognized as a row at
  all, so both rows reach nothing. The whole `Arguments:` block around them
  parses, and 43 dash-led rows in it come out correct. The two rows are real
  and documented. Open defect, recorded by `corpus/vim.basic/audit-seed4`.
  Distinct from S-086, which is the plus-or-minus alternation notation.
- fleet: not measured

### S-096: end-of-options marker row dropped

- id: S-096
- looks like: |
      --			Only file names after this
- tools: vim.basic
- handling: A row whose whole name is a bare double dash is deliberately left eligible
  as a row (S-090 refuses only three dashes or more), and it still fails to
  reach the tree. The marker is real and the row carries its own description.
  Open defect, recorded by `corpus/vim.basic/audit-seed4`.
- fleet: not measured

### S-097: glued optional value spec loses everything after the first bracket

- id: S-097
- looks like: |
      -V[N][fname]		Be verbose [level N] [log messages to fname]
- tools: vim.basic
- handling: A value spec written as two bracket groups glued to the flag keeps only the
  first. The flag reaches the tree spelled `-V` with the optional value `N`,
  and `fname` is rendered nowhere. The row's description still names it, so
  the loss is in the value alone. Open defect, recorded by
  `corpus/vim.basic/audit-seed4`.
- fleet: not measured

### S-098: parenthetical qualifier read as a value and truncated

- id: S-098
- looks like: |
      -r			List swap files and exit
      -r (with file name)	Recover crashed session
- tools: vim.basic
- handling: A tool documents one spelling twice and qualifies the second row in
  parentheses. The qualifier is read as a value placeholder and cut at the
  first space, so the flag carries the value `(with` and the words `file
  name)` are rendered nowhere. Open defect, recorded by
  `corpus/vim.basic/audit-seed4`.
- fleet: not measured

### S-099: alias pair joined by the word "or"

- id: S-099
- looks like: |
      -h  or  --help	Print Help (this message) and exit
- tools: vim.basic
- handling: A row that joins two spellings with the word `or` instead of a comma yields
  one flag `-h` whose description begins `or --help`. The `--help` spelling
  never becomes an alias, and the description carries text that is not
  description. Open defect, recorded by `corpus/vim.basic/audit-seed4`.
- fleet: not measured

### S-100: usage alternative reaches the tree without its own description

- id: S-100
- looks like: |
         or: vim [arguments] -t tag          edit file where tag is defined
         or: vim [arguments] -q [errorfile]  edit file with first error
- tools: vim.basic
- handling: An alternative synopsis line names a flag and describes it on the same line.
  The flag reaches the tree from the synopsis with no description, and `-q`
  arrives with no value placeholder either. The text is present and aligned in
  a description column. Open defect, recorded by
  `corpus/vim.basic/audit-seed4`.
- fleet: not measured

### S-101: repetition dots glued to an operand name

- id: S-101
- looks like: |
      Usage: aarch64-linux-gnu-dwp [options] [file...]
      usage: bc [options] [file ...]
- tools: aarch64-linux-gnu-dwp, dwp, demandoc, lsattr, lsinitramfs
- handling: A shared predicate marks an operand repeatable when it ends in two or
  more dots after trimming a trailing `]` or `)`. It backs both the main
  positional loop and the tail-operand recovery path. The operand reaches the
  tree named `file` and repeatable, matching `corpus/aarch64-linux-gnu-dwp/2.42`.
- fleet: 11 tools gained the marker in a direct before/after diff over captured bytes,
  2026-09-03. Zero tools changed any other way and zero of 38 checked controls moved.

### S-102: one spelling documented on two rows, folded before display

- id: S-102
- looks like: |
      -r			List swap files and exit
      -r (with file name)	Recover crashed session
- tools: vim.basic, ffplay, ffmpeg, curl, gcc, as, dpkg, tar, du, expand, lsof,
  update-xmlcatalog, rust-lldb, qemu-arm64-static, ptargrep, pod2man,
  mkfs.bfs, man-recode
- handling: A tool may document one spelling on two rows for two forms, and the parser
  extracts both. The interactive display then folds them into one entity,
  keyed on spelling alone, and picks the value name and the description from
  different members. One row's description reaches no rendered surface. The
  extraction is correct and the loss happens after it. Open defect.
- fleet: 18 of 133 corpus fixtures carry a spelling on two entities with
  different descriptions, 2026-09-03. Measured loss between the extracted
  count and the rendered count: ffplay 1136 to 465, gcc 43 to 37, curl 258
  to 253, as 61 to 59, vim.basic 45 to 43, tar 159 to 157, du 29 to 28,
  expand 5 to 4.

### S-103: a command's wrapped description invents a subcommand

- id: S-103
- looks like: |
        ls, list                 Print all the versions of packages that are
                                 installed, as well as their dependencies, in a
                                 tree-structure
- tools: pnpm
- handling: A command's description wraps onto a physical line with no column of
  its own. The generic layout parser's heading fallback misreads an earlier
  ragged-indent row as a section heading (see S-104), and the orphaned
  continuation's own leading word then reads as a fresh subcommand.
  `scan_ragged_command_run` recognizes the owning row directly, before the
  heading fallback ever sees it, so the continuation folds into that row's
  description instead of becoming a node. Fixed.
- fleet: 0 after the fix (pnpm's 2 inventions, `tree-structure` and `package`,
  both gone), 2026-09-03. The `wrapped-command-continuation-as-subcommand`
  detector's own fleet-wide count is reported, not gated: a full sweep found
  1 unrelated tool it also fires on, not yet excluded by a declared scope.

### S-104: a short-alias prefix ragged-indents a command table's rows

- id: S-104
- looks like: |
        add                  Installs a package and any packages that it depends
     i, install              Install all dependencies for a project
    ln, link                 Connect the local project to another one
- tools: pnpm
- handling: A command table's rows carry an optional short-alias-comma prefix
  (`i, install`), which sits at a shallower indent than the unaliased rows
  around it. The generic layout parser's block scanners key a row-vs-
  continuation decision on one fixed indent baseline per block, which cannot
  admit both indents at once: the shallower row never opens a block and is
  dropped, and the deeper siblings after it get swallowed by the heading
  fallback (see S-103). `scan_ragged_command_row`/`scan_ragged_command_run`
  recognize each row directly, one physical line at a time, gated on the
  document already being in a command-listing context and on a run of 2+
  such rows in strict adjacency. Fixed.
- fleet: 0 after the fix (pnpm's 12 aliased-and-sibling rows all recovered:
  `i, install`, `ln, link`, `rm, remove`, `unlink`, `up, update`, `ls, list`,
  `outdated`, `why`, `c, config`, `init`, `publish`, `stage`), 2026-09-03. The
  `ragged-command-table` detector's own fleet-wide count is reported, not
  gated: a full sweep found it also fires on 144 tools whose "missing
  command" is really an unrelated bullet or reference list, not this shape.
