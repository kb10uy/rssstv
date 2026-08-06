# MMJASTA

`original/mmsstv/JASTA` is a separate Borland C++ Builder application, not part
of MMSSTV proper. It scores the JASTA SSTV Activity Contest from a log MMSSTV
recorded, and writes the log sheet, summary sheet, and an analysis file the
entrant mails to the contest secretary. It shares its lower layers with MMSSTV
by file copy rather than by reference: `ComLib`, `LogFile`, `LogConv`,
`country`, and `ARRL.DX` all exist in both trees and have diverged.

Nothing in RSSSTV corresponds to this program; RSSSTV has no logging. The part
worth keeping is the description of the MDT log file and the DXCC prefix
matcher, because both are MMSSTV formats that a log feature would have to read.

## Source Map

- `JASTA.txt`: the 2010 contest rules, English then Japanese. This is the
  authority the program is measured against. It is not the current rule set:
  the contest still runs, and by the 47th (2026) edition WARC bands are
  excluded, a QSO with a station that sent no self-portrait counts, and the log
  deadline moved to September 10. JASTA has also moved twice since, from
  `homepage3.nifty.com` through `sstv.image.coocan.jp` to
  <https://ja2hyd.main.jp/jasta/>. MMJASTA implements the 2010 text.
- `JASTA/mmjasta.txt`, `JASTA/EMMJASTA.TXT`: the operator manual.
- `JASTA/Main.cpp:264-314`: `Exec()`, load and filter.
- `JASTA/Main.cpp:316-329`: `IsValidRST()`, serial-number validation and
  normalization.
- `JASTA/Main.cpp:331-388`: `AdjustData()`, per-QSO points and multiplier.
- `JASTA/Main.cpp:409-647`: `Calc()`, scoring and both output sheets.
- `JASTA/Main.cpp:649-676`: `GetBNO()`, band-to-column mapping.
- `JASTA/Main.cpp:679-1070`: `MakeANA()`, the analysis file.
- `JASTA/Main.cpp:1181-1234`: manual multiplier editing and write-back.
- `JASTA/Main.cpp:1248-1366`: ADIF, Log200, and Turbo HAMLOG import.
- `JASTA/LogFile.h:14-42`: the band enumeration.
- `JASTA/LogFile.h:90-126`: the MDT record and header structures.
- `JASTA/LogFile.cpp:119-155`: index reading.
- `JASTA/LogFile.cpp:458-624`: open, read, write.
- `JASTA/LogFile.cpp:1369-1419`: JST to UTC conversion.
- `JASTA/country.cpp:91-108`: the DXCC prefix pattern matcher.
- `JASTA/ComLib.cpp:877-934`: `IsJA()`.
- `JASTA/ComLib.cpp:1038-1058`: `ClipCC()`, extraction of the prefix-bearing
  segment of a portable callsign.

## The MDT Log File

An MDT file is a flat array of fixed records behind a fixed header.

The header is `FHD`, 240 bytes with packing disabled, but records begin at the
constant offset `FHDOFF` = 256. It opens with the 20-byte signature
`"MMLOG DATA Ver1.00\032"`, which `CLogFile::Open()` compares exactly. `size`
is the record count, `mlt` the byte length of a trailing multiplier blob,
`hash` the index format, and `mode[32][6]` a table of user-defined mode names
for records whose mode byte is at or above `MODEMAX` (48).

`SDMMLOG` is exactly 256 bytes, which is why the record stride and the header
offset are the same number. Its fields:

- `year`: two-digit year as a byte. `YEAR()` pivots at 50, so 0-49 mean
  2000-2049.
- `date`: `month * 100 + day`.
- `btime`, `etime`: time of day in units of 1/30 minute, that is two seconds.
  Hours are `btime / 30 / 60`.
- `call`, `ur`, `my`: callsign, the report sent, the report received. The
  names read backwards: `ur` is what you gave the other station and `my` is
  what you got.
- `band`: an index into the enumeration, not a frequency.
- `mode`: an index into a fixed mode table; 8 is SSTV.
- `qsl`, `rem`, `opt1`, `opt2`: free text in normal use.

After the records comes the callsign index and then the multiplier blob. A
`hash` of 2 means a 16-byte-per-record index of callsigns, which is read
directly; a `hash` of 1 means an older two-byte index, which is skipped and
then rebuilt by `CIndex::MakeIndex()`.

The band enumeration is ordered by neither frequency nor era: `B_4630`,
`B_220`, and `B_SAT` were appended after `B_248G`. Code that compares band
indices to decide anything about frequency is wrong for those three, and
MMJASTA does exactly that.

## Load and Filter

`Exec()` copies qualifying records out of the source log into a scratch file
`JASTA.$$$` and scores that, leaving the source read-only. A record qualifies
when all of the following hold, after `JSTtoUTC()` has been applied:

- `mode == 8`, SSTV.
- `band >= B_35`, which by enum order excludes only 1.9 MHz.
- the year equals the configured contest year.
- the month is August.
- `IsValidRST(ur)` succeeds, that is a contest number was sent.

`IsValidRST()` requires at least four characters, all digits. It then
normalizes the serial in place: if the value after the third character is below
1000 and the string is not already six characters, the serial is rewritten
zero-padded to three digits, so `59901` becomes `599001`. Four-digit serials
are left alone.

The surviving records are sorted by year, date, and time.

`JSTtoUTC()` is applied unconditionally and subtracts a hard nine hours. There
is no consultation of `LOGSET::m_TimeZone`. A log kept in UTC is shifted by
minus nine hours, which moves QSOs across the August boundary and across the
day boundary the duplicate check depends on.

The contest year defaults to the current year, minus one if the current month
is before August, and is not derived from the log.

## Field Repurposing

Scoring does not use a separate table. It overwrites three text fields of the
scratch copy and reads them back later:

- `qsl` becomes the QSO point value as a decimal string: `"1"`, `"2"`, `"3"`,
  or `"0"` for a QSO that does not count. The grid column labeled Point reads
  it.
- `opt1` becomes the multiplier: `JA1` through `JA0` for Japanese stations, or
  the DXCC prefix otherwise. A leading `*` marks a rejected QSO, one of
  `*INV*`, `*NOF*`, or `*DUP*`.
- `opt2` becomes the continent.

Points follow the band index: at or below `B_28` scores 1, at or below `B_430`
scores 2, anything else scores 3. Because of the enum ordering noted above,
4630 kHz, 220 MHz, and satellite QSOs score 3.

The Japanese district is taken from `ClipCC()` of the callsign: if the first
character is `7` and the second is not `J`, the district is 1, per the rule
that 7K through 7N are all JA1; otherwise the district is the third character.
Everything else goes to the DXCC lookup.

A QSO is rejected when the received report fails `IsValidRST()` (`*INV*`), or
when `rem` contains `NOF` or `nof` in any case combination the two `strstr`
calls happen to cover (`*NOF*`). Both are set during load. Duplicates are
detected during scoring.

### The Edit-Lock Sentinel

When the operator corrects a multiplier by hand, the correction has to survive
the next re-totalization, which re-runs `AdjustData()`. The flag is stored
inside the option field itself, past the terminator:

```c
LPSTR p = lastp(sd.opt1);   // last character, not the terminator
p += 2;                     // one byte past the NUL
*p = 0x01;
```

`AdjustData()` computes the same address and skips the automatic overwrite when
it finds `0x01` there.

`opt1` and `opt2` are nine bytes each and adjacent in the struct. For an
eight-character string the sentinel address is index 9: for `opt1` that is
`opt2[0]`, so writing the flag overwrites the first character of the continent
and reading it tests the continent instead of a flag. Eight-character values
are reachable, because `StrCopy(sp->opt1, pDX, MLOPT)` truncates entity names
to exactly eight characters. For `opt2` the same case writes one byte past the
end of the record structure.

## Scoring

The multiplier is the sum of three counts: distinct Japanese districts,
distinct DXCC entities excluding Japan, and the number of distinct UTC days,
capped at 10. Days are counted by watching `date % 100` change across the
date-sorted log. The cap applies only to the multiplier; the log sheet header
prints the uncapped count.

The score is the sum of QSO points, and the total is their product. This
matches the published rules.

Duplicates are rejected per UTC day regardless of band, with the day's set
cleared when the date changes. The key is the logged callsign after whitespace
removal, so `JA1ABC` and `JA1ABC/3` are two stations. QSOs already rejected as
`*INV*` or `*NOF*` never enter the duplicate set, so a later contact with the
same station on the same day is scored as the first.

Two conditions are reported as warnings but still scored: a non-Japanese
station worked on 144 MHz or above, and a multiplier that could not be
resolved. The latter blocks the output sheets from opening and asks the
operator to fix it.

## DXCC Resolution

`ARRL.DX` is a semicolon-delimited text file loaded from the program's own
directory, at most 512 entries, `!` comments, `$` terminating. The `CTL` member
names do not match the columns: the first column is the canonical prefix and is
stored in `Name`, and the country name is stored in `QTH`. The multiplier
written into `opt1` is `Name`, so it is the prefix, not the country.

The second column is a comma-separated pattern list. `country.cpp:91-108`
matches a callsign against a pattern where `*` skips ahead to the next literal
match, `?` matches any single character, and `\` asserts end of string. A
pattern of the form `AP-AS` is a range: `lcmpp()` finds the first differing
character position and the matcher iterates that character from the low bound
to the high. Matching is attempted twice, first requiring the pattern to
consume the whole callsign and then allowing a trailing remainder.

Lookup tries the full callsign first and then `ClipCC()` of it, which returns
the slash-delimited segment that carries a digit, falling back to the segment
that is neither `Q`-initial nor `MM`.

A multiplier is treated as unresolved when it is empty, contains `?`, is
exactly `JA`, or is all digits. The operator then edits it in a dialog, and the
correction is written back to every QSO with the same callsign in both the
scratch log and the source log. When the source was ADIF, Log200, or HAMLOG,
the write-back target is the intermediate `MMJASTA.MDT` rather than the
original file, which is what the manual describes.

## Import Formats

ADIF, Log200, and Turbo HAMLOG inputs are converted to a `MMJASTA.MDT`
intermediate first, keeping only SSTV records at 3.5 MHz and above and clearing
both option fields. HAMLOG keeps the report and the serial in separate places,
so `AdjustHamlogRSV()` appends a three-digit serial recovered from the remarks
fields onto each report when the report is three characters or shorter.

## The Analysis File

`MakeANA()` writes per-callsign QSO counts, then a sequence of band-by-category
matrices: QSOs per Japanese district, per DXCC entity, per continent, per UTC
day and per UTC hour for all, DX-only and JA-only, and new multipliers per day
and per hour. It is a working aid, not something the contest asks for.

`GetBNO()` maps band indices onto the twelve columns the headers describe as
3.5, 7, 14, 18, 21, 24, 28, 50, 144, 430, 1200, 2400+. Three defects:

- `b >= B_2400` maps to column 9, which is the 430 MHz column. 2400 MHz and up
  is counted as 430 MHz. The comment above the function lists twelve bands, so
  the intended value is 11.
- The fallback for bands below 14 MHz other than 3.5, 3.8, and 7 maps to column
  11. Since the 2400+ case never reaches it, column 11 holds only 10 MHz QSOs.
- The loops that set each column's `all` total and that clear the accumulators
  between matrices run `b < 11` while the printing loops run `b < 12`. Column 11
  therefore never receives a total and is never cleared, so counts leak from one
  matrix into the next.

## Other Notes

Everything about the entrant — callsign, category, first-entry flag, OM or YL,
T-shirt size, address, license class, power — comes from `MMJASTA.INI`, not from
the log. The category and the output language default from the Windows locale on
first run.

The warning pane is a fixed 32768-byte buffer written through unchecked
`vsprintf`. The only guard is the hundred-message cap in `ShowErr()`.

Sources are Shift_JIS and the strings are VCL `AnsiString`, so anything reading
this code has to decode explicitly.
