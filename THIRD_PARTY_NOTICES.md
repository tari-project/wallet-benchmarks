# Third-Party Provenance

The repository's BSD-3-Clause license covers the wallet-benchmark harness and
other original material contributed here. It does not relicense third-party
projects fetched by the build scripts.

- Tari and `minotari-cli` are fetched from the exact revisions recorded in
  `tools/build-manifest.json`; their upstream repositories carry their own
  BSD-3-Clause licensing and notices.
- `minotari_payment_processor` is fetched from the exact revision recorded in
  the manifest. At that revision, its repository and crate metadata do not
  declare a license. No payment-processor source tree or binary is distributed
  here. The small tracked fee-rate patch exists solely to make the bounty's
  required `5 uT` setting explicit and reproducible; its presence does not imply
  that upstream payment-processor code is covered by this repository's license.
  Users and redistributors must obtain any permission required from the upstream
  maintainers.
