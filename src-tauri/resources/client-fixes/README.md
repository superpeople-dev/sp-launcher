# Current-build HUD adaptation

Contains current 1.3.0.473797 UW-MainWidget and UW-TopInfoWidget package pairs,
plus the English CheatTable pair. No older Blueprint assets are bundled.

The health container matches the current progress widget's native 300-unit
maximum width and 30-unit slot size. Health numbers use centre anchors instead
of a fixed horizontal offset, with public automatic sizing, vertical centring,
size 12 current/max health and size 10 slash. Equipment and boost keep their
accepted spacing relative to the bar. The existing stance switcher is visible
above its left end and uses the public positive 0.7 scale. Its four state slots,
images and update logic remain unchanged.

The right panel retains public 170 x 79 bounds, damage placement and weapon
icon/name/grade grouping. Ammo and weapon panels sit 20 units further left to
meet the actual health-bar edge. The public ammo dimensions and fire mode,
loaded rounds and reserve ordering remain unchanged.

All 18,500 TopInfo exports parse. This final pass changes six presentation
exports; the other 18,494 exports, including all 65 Blueprint functions, are
byte-for-byte unchanged from the accepted right-side build. MainWidget is also
byte-identical. No class, object-name or preload-dependency changes are made.
final-tweaks-changes.json and final-tweaks-verification.json record this pass.
serialization-verification.json additionally checks exact native widget property
boundaries and preserves the source property schema. Stance Visibility must use
EnumProperty; a ByteProperty substitution leaves trailing data and causes the
game to reject the package. The strict check rejects that malformed candidate.
Earlier reports describe their respective historical candidates.

The signed PAK is 434,883 bytes, Oodle Kraken, Normal, 64 KiB blocks. All six
payloads, compression blocks and encrypted indices pass read-back verification.
payload-report.json records hashes; GitHub Actions verifies packaged hashes and
runs the DLL's signature validation before building. No binary parts or private
signing key are included.

Enable Apply client fixes to deploy the signed pair for the game session.
The prior left/right layout was confirmed in game. These final adjustments
still need verification with changing health and standing/crouching/prone.
Offline checks establish geometry and serialization, not runtime stance updates.
The main branch includes this HUD bundle alongside the cheat-menu translations.
