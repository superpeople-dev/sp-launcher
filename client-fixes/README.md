# Client fixes

**Client fixes** in the launcher's Settings is on by default and loads this DLL. The
**Client fixes debug window** option controls its diagnostic console on the
next game launch and is off by default. Diagnostics still go to an attached
debugger when the window is hidden.

- **Local class selection:** Unlocks all current classes in standalone local
  games by temporarily raising the local player's class eligibility level to
  5. Online matches are excluded.
- **White and Gold Super Capsules:** Corrects their merged item table buff IDs
  so both capsules can be used. The DLL verifies the original IDs and the
  replacement buff rows before changing either value, then reapplies the fix
  if the table is rebuilt.
- **First Blood audio:** Lets the first cue play in a local match, suppresses
  later bot first-kill cues, and re-arms it for the next match. It checks the
  loaded perk widget script and sound assets before changing the audio
  reference. The 25 ms widget poll can miss exceptionally close kills.
- **Cheat Widget translation:** Replaces all 69 Korean command descriptions
  in the loaded `CheatTable` with concise English labels. It checks the table
  and row layout before writing, stays within each existing string buffer,
  and repeats when the table reloads. **Close and reopen the Cheat Widget in
  game for the English labels to appear.** The separate yellow Close button
  remains Korean.
