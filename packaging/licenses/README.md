# Supplemental upstream license texts

Some Cargo archives omit their upstream license files. These unchanged snapshots fill those gaps in the Debian package's third-party notices without requiring network access during collection. They retain their upstream licenses, independently of RODE's MIT license.

`manifest.json` records the applicable package/version pairs, exact upstream source URLs and SHA-256 hashes. Upstream commits match the `.cargo_vcs_info.json` shipped in those crate versions. The builder verifies every used snapshot and collects the crate's own notices too, including egui's separate font licenses and REUSE license directories/copyright headers.

When updating dependencies, review the license files at the new crate's upstream revision, update the applicable manifest entries and snapshots, then build a package and run `python3 scripts/test-deb.py <package.deb>`. Do not substitute a generic SPDX identifier for an upstream copyright/permission notice. Snapshots are build inputs; their complete contents and source URLs appear in `THIRD-PARTY-NOTICES.gz`.
