# Third-Party Notices

Desk Switch includes or links to the third-party packages and components listed below. This file is generated from `cargo metadata --format-version 1 --locked`; the offline freshness check compares its package markers with `Cargo.lock`. SPDX license expressions, repository/homepage URLs, and authors are reproduced from Cargo metadata. An SPDX expression that offers alternatives describes the upstream licensing options; it does not assert that every option is used. Package-specific copyright and NOTICE texts remain part of their respective notices.

To regenerate from the repository root, run:

```console
python tools/gen_third_party_notices.py
```

To check the committed inventory without Cargo or network access, run `python tools/gen_third_party_notices.py --check`.

The resolved graph contains **503 third-party Cargo packages**. The root package `desk-switch` is excluded from that count. Additional non-Cargo bundled components are listed separately.

## Desk Switch License

```text
MIT License

Copyright (c) 2026 Desk Switch contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Additional Bundled Components

### m1ddc

- **Version:** Not versioned in the vendored source snapshot
- **License:** MIT; the exact vendored text is reproduced in Appendix C
- **Repository:** https://github.com/waydabber/m1ddc
- **Source:** `vendor/m1ddc`; used by the macOS Apple Silicon DDC transport

### VB-CABLE Virtual Audio Device

The original VB-Audio installer archives are separate donationware, not part of Desk Switch's MIT-licensed code. The vendored redistribution notice and archive hashes are reproduced verbatim in Appendix C.

- **Website:** https://www.vb-cable.com/
- **Source:** `assets/third-party/vb-cable`

## Resolved Cargo Packages

### accesskit 0.24.1

<!-- cargo-package: accesskit 0.24.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/AccessKit/accesskit
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The AccessKit contributors

### adler2 2.0.1

<!-- cargo-package: adler2 2.0.1 -->

- **License (Cargo metadata):** `0BSD OR MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/oyvindln/adler2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jonas Schievink <jonasschievink@gmail.com>; oyvindln <oyvindln@users.noreply.github.com>

### aead 0.5.2

<!-- cargo-package: aead 0.5.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/traits
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### ahash 0.8.12

<!-- cargo-package: ahash 0.8.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/tkaitchuck/ahash
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Tom Kaitchuck <Tom.Kaitchuck@gmail.com>

### ambient-authority 0.0.2

<!-- cargo-package: ambient-authority 0.0.2 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/ambient-authority
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### android-activity 0.6.1

<!-- cargo-package: android-activity 0.6.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-mobile/android-activity
- **Homepage:** https://github.com/rust-mobile/android-activity

### android-properties 0.2.2

<!-- cargo-package: android-properties 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/miklelappo/android-properties
- **Homepage:** https://github.com/miklelappo/android-properties
- **Authors:** Mikhail Lappo <mikhail.lappo@esrlabs.com>

### anyhow 1.0.104

<!-- cargo-package: anyhow 1.0.104 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/anyhow
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### arboard 3.7.0

<!-- cargo-package: arboard 3.7.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/1Password/arboard
- **Homepage:** Not specified in Cargo metadata

### arc-swap 1.9.3

<!-- cargo-package: arc-swap 1.9.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/vorner/arc-swap
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Michal 'vorner' Vaner <vorner@vorner.cz>

### arrayvec 0.7.8

<!-- cargo-package: arrayvec 0.7.8 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bluss/arrayvec
- **Homepage:** Not specified in Cargo metadata
- **Authors:** bluss

### as-raw-xcb-connection 1.0.1

<!-- cargo-package: as-raw-xcb-connection 1.0.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/psychon/as-raw-xcb-connection
- **Homepage:** Not specified in Cargo metadata

### ashpd 0.13.13

<!-- cargo-package: ashpd 0.13.13 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/bilelmoussaoui/ashpd
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Bilal Elmoussaoui <bil.elmoussaoui@gmail.com>

### async-broadcast 0.7.2

<!-- cargo-package: async-broadcast 0.7.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/async-broadcast
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Stjepan Glavina <stjepang@gmail.com>; Yoshua Wuyts <yoshuawuyts@gmail.com>; Zeeshan Ali Khan <zeeshanak@gnome.org>

### async-recursion 1.2.0

<!-- cargo-package: async-recursion 1.2.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dcchut/async-recursion
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Robert Usher <266585+dcchut@users.noreply.github.com>

### async-trait 0.1.92

<!-- cargo-package: async-trait 0.1.92 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/async-trait
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### atomic-waker 1.1.2

<!-- cargo-package: atomic-waker 1.1.2 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/atomic-waker
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Stjepan Glavina <stjepang@gmail.com>; Contributors to futures-rs

### autocfg 1.5.1

<!-- cargo-package: autocfg 1.5.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/cuviper/autocfg
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Josh Stone <cuviper@gmail.com>

### base64 0.22.1

<!-- cargo-package: base64 0.22.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/marshallpierce/rust-base64
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Marshall Pierce <marshall@mpierce.org>

### base64 0.23.1

<!-- cargo-package: base64 0.23.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/marshallpierce/rust-base64
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Marshall Pierce <marshall@mpierce.org>

### base64ct 1.8.3

<!-- cargo-package: base64ct 1.8.3 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/formats
- **Homepage:** https://github.com/RustCrypto/formats/tree/master/base64ct
- **Authors:** RustCrypto Developers

### bincode 1.3.3

<!-- cargo-package: bincode 1.3.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/servo/bincode
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Ty Overby <ty@pre-alpha.com>; Francesco Mazzoli <f@mazzo.li>; David Tolnay <dtolnay@gmail.com>; Zoey Riordan <zoey@dos.cafe>

### bit-set 0.9.1

<!-- cargo-package: bit-set 0.9.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/contain-rs/bit-set
- **Homepage:** https://github.com/contain-rs/bit-set
- **Authors:** Alexis Beingessner <a.beingessner@gmail.com>

### bit-vec 0.9.1

<!-- cargo-package: bit-vec 0.9.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/contain-rs/bit-vec
- **Homepage:** https://github.com/contain-rs/bit-vec
- **Authors:** Alexis Beingessner <a.beingessner@gmail.com>

### bitflags 1.3.2

<!-- cargo-package: bitflags 1.3.2 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bitflags/bitflags
- **Homepage:** https://github.com/bitflags/bitflags
- **Authors:** The Rust Project Developers

### bitflags 2.13.2

<!-- cargo-package: bitflags 2.13.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bitflags/bitflags
- **Homepage:** https://github.com/bitflags/bitflags
- **Authors:** The Rust Project Developers

### bitvec 1.1.1

<!-- cargo-package: bitvec 1.1.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/bitvecto-rs/bitvec
- **Homepage:** https://bitvecto-rs.github.io/bitvec

### blake2 0.10.6

<!-- cargo-package: blake2 0.10.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/hashes
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### block-buffer 0.10.4

<!-- cargo-package: block-buffer 0.10.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/utils
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### block2 0.5.1

<!-- cargo-package: block2 0.5.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Sheldon; Mads Marquart <mads@marquart.dk>

### block2 0.6.2

<!-- cargo-package: block2 0.6.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Mads Marquart <mads@marquart.dk>

### bumpalo 3.20.3

<!-- cargo-package: bumpalo 3.20.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/fitzgen/bumpalo
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Nick Fitzgerald <fitzgen@gmail.com>

### bytemuck 1.25.2

<!-- cargo-package: bytemuck 1.25.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Lokathor/bytemuck
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Lokathor <zefria@gmail.com>

### bytemuck_derive 1.12.1

<!-- cargo-package: bytemuck_derive 1.12.1 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Lokathor/bytemuck
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Lokathor <zefria@gmail.com>

### byteorder-lite 0.1.0

<!-- cargo-package: byteorder-lite 0.1.0 -->

- **License (Cargo metadata):** `Unlicense OR MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/image-rs/byteorder-lite
- **Homepage:** https://github.com/image-rs/byteorder-lite

### bytes 1.12.1

<!-- cargo-package: bytes 1.12.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/bytes
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Carl Lerche <me@carllerche.com>; Sean McArthur <sean@seanmonstar.com>

### calloop 0.13.0

<!-- cargo-package: calloop 0.13.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/Smithay/calloop
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### calloop 0.14.5

<!-- cargo-package: calloop 0.14.5 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/Smithay/calloop
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### calloop-wayland-source 0.3.0

<!-- cargo-package: calloop-wayland-source 0.3.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/calloop-wayland-source
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### calloop-wayland-source 0.4.1

<!-- cargo-package: calloop-wayland-source 0.4.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/calloop-wayland-source
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### cap-fs-ext 4.0.3

<!-- cargo-package: cap-fs-ext 4.0.3 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/cap-std
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>; Jakub Konka <kubkon@jakubkonka.com>
- **Vendored license texts:**
  - `assets/third-party/file-transfer/cap-std-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### cap-primitives 4.0.3

<!-- cargo-package: cap-primitives 4.0.3 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/cap-std
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>; Jakub Konka <kubkon@jakubkonka.com>

### cap-std 4.0.3

<!-- cargo-package: cap-std 4.0.3 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/cap-std
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>; Jakub Konka <kubkon@jakubkonka.com>
- **Vendored license texts:**
  - `assets/third-party/file-transfer/cap-std-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### cc 1.11.0

<!-- cargo-package: cc 1.11.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/cc-rs
- **Homepage:** https://github.com/rust-lang/cc-rs

### cfg-if 1.0.5

<!-- cargo-package: cfg-if 1.0.5 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/cfg-if
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Alex Crichton <alex@alexcrichton.com>

### cfg_aliases 0.2.2

<!-- cargo-package: cfg_aliases 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/katharostech/cfg_aliases
- **Homepage:** https://github.com/katharostech/cfg_aliases
- **Authors:** Zicklag <zicklag@katharostech.com>
- **Upstream NOTICE files:**
  - `NOTICES.md` (reproduced below)

#### Upstream `NOTICES.md`

```text
# 3rd Party Notices

The `cfg_aliases!` macro uses a lot of the code from [`tectonic_cfg_support::target_cfg!`] macro which is under the following license:

[`tectonic_cfg_support::target_cfg!`]: https://github.com/tectonic-typesetting/tectonic/blob/f2439b936470ad27bdf92882064bc4702ee01899/cfg_support/src/lib.rs#L166

    tectonic_cfg_support is licensed under the MIT License.

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the “Software”), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.
---
```

### cgl 0.3.2

<!-- cargo-package: cgl 0.3.2 -->

- **License (Cargo metadata):** `MIT / Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/cgl-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Servo Project Developers

### chacha20 0.10.2

<!-- cargo-package: chacha20 0.10.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/stream-ciphers
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### chacha20 0.9.1

<!-- cargo-package: chacha20 0.9.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/stream-ciphers
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### chacha20poly1305 0.10.1

<!-- cargo-package: chacha20poly1305 0.10.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/AEADs/tree/master/chacha20poly1305
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### cipher 0.4.4

<!-- cargo-package: cipher 0.4.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/traits
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### clipboard-win 5.4.3

<!-- cargo-package: clipboard-win 5.4.3 -->

- **License (Cargo metadata):** `BSL-1.0`
- **Repository:** https://github.com/DoumanAsh/clipboard-win
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Douman <douman@gmx.se>

### codespan-reporting 0.13.1

<!-- cargo-package: codespan-reporting 0.13.1 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/brendanzab/codespan
- **Homepage:** https://github.com/brendanzab/codespan
- **Authors:** Brendan Zabarauskas <bjzaba@yahoo.com.au>

### color 0.3.3

<!-- cargo-package: color 0.3.3 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/color
- **Homepage:** Not specified in Cargo metadata

### combine 4.6.9

<!-- cargo-package: combine 4.6.9 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/Marwes/combine
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Markus Westerlind <marwes91@gmail.com>

### concurrent-queue 2.5.0

<!-- cargo-package: concurrent-queue 2.5.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/concurrent-queue
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Stjepan Glavina <stjepang@gmail.com>; Taiki Endo <te316e89@gmail.com>; John Nunley <dev@notgull.net>

### const-oid 0.9.6

<!-- cargo-package: const-oid 0.9.6 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/formats/tree/master/const-oid
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### constant_time_eq 0.6.1

<!-- cargo-package: constant_time_eq 0.6.1 -->

- **License (Cargo metadata):** `CC0-1.0 OR MIT-0 OR Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://codeberg.org/cesarb/constant_time_eq
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Cesar Eduardo Barros <cesarb@cesarb.eti.br>

### constify 0.0.1

<!-- cargo-package: constify 0.0.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/DGriffin91/egui_software_backend
- **Homepage:** https://github.com/DGriffin91/egui_software_backend

### core-foundation 0.9.4

<!-- cargo-package: core-foundation 0.9.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/core-foundation-rs
- **Homepage:** https://github.com/servo/core-foundation-rs
- **Authors:** The Servo Project Developers

### core-foundation-sys 0.8.7

<!-- cargo-package: core-foundation-sys 0.8.7 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/core-foundation-rs
- **Homepage:** https://github.com/servo/core-foundation-rs
- **Authors:** The Servo Project Developers

### core-graphics 0.23.2

<!-- cargo-package: core-graphics 0.23.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/core-foundation-rs
- **Homepage:** https://github.com/servo/core-foundation-rs
- **Authors:** The Servo Project Developers

### core-graphics-types 0.1.3

<!-- cargo-package: core-graphics-types 0.1.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/core-foundation-rs
- **Homepage:** https://github.com/servo/core-foundation-rs
- **Authors:** The Servo Project Developers

### cpufeatures 0.2.17

<!-- cargo-package: cpufeatures 0.2.17 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/utils
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### cpufeatures 0.3.1

<!-- cargo-package: cpufeatures 0.3.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/utils
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### crc32fast 1.5.4

<!-- cargo-package: crc32fast 1.5.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/srijs/rust-crc32fast
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Sam Rijs <srijs@airpost.net>; Alex Crichton <alex@alexcrichton.com>

### crossbeam-channel 0.5.17

<!-- cargo-package: crossbeam-channel 0.5.17 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/crossbeam-rs/crossbeam
- **Homepage:** https://github.com/crossbeam-rs/crossbeam/tree/master/crossbeam-channel

### crossbeam-queue 0.3.14

<!-- cargo-package: crossbeam-queue 0.3.14 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/crossbeam-rs/crossbeam
- **Homepage:** https://github.com/crossbeam-rs/crossbeam/tree/master/crossbeam-queue

### crossbeam-utils 0.8.23

<!-- cargo-package: crossbeam-utils 0.8.23 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/crossbeam-rs/crossbeam
- **Homepage:** https://github.com/crossbeam-rs/crossbeam/tree/master/crossbeam-utils

### crunchy 0.2.4

<!-- cargo-package: crunchy 0.2.4 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/eira-fransham/crunchy
- **Homepage:** https://github.com/eira-fransham/crunchy
- **Authors:** Eira Fransham <jackefransham@gmail.com>

### crypto-common 0.1.7

<!-- cargo-package: crypto-common 0.1.7 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/traits
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### cursor-icon 1.2.0

<!-- cargo-package: cursor-icon 1.2.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR Zlib`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/cursor-icon
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### curve25519-dalek 4.1.3

<!-- cargo-package: curve25519-dalek 4.1.3 -->

- **License (Cargo metadata):** `BSD-3-Clause`
- **Repository:** https://github.com/dalek-cryptography/curve25519-dalek/tree/main/curve25519-dalek
- **Homepage:** https://github.com/dalek-cryptography/curve25519-dalek
- **Authors:** Isis Lovecruft <isis@patternsinthevoid.net>; Henry de Valence <hdevalence@hdevalence.ca>

### curve25519-dalek-derive 0.1.1

<!-- cargo-package: curve25519-dalek-derive 0.1.1 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dalek-cryptography/curve25519-dalek
- **Homepage:** https://github.com/dalek-cryptography/curve25519-dalek

### der 0.7.10

<!-- cargo-package: der 0.7.10 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/formats/tree/master/der
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### digest 0.10.7

<!-- cargo-package: digest 0.10.7 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/traits
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### dirs 6.0.0

<!-- cargo-package: dirs 6.0.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/soc/dirs-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Simon Ochsenreither <simon@ochsenreither.de>

### dirs-sys 0.5.0

<!-- cargo-package: dirs-sys 0.5.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dirs-dev/dirs-sys-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Simon Ochsenreither <simon@ochsenreither.de>

### dispatch 0.2.0

<!-- cargo-package: dispatch 0.2.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** http://github.com/SSheldon/rust-dispatch
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Sheldon

### dispatch2 0.3.1

<!-- cargo-package: dispatch2 0.3.1 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Mads Marquart <mads@marquart.dk>; Mary <mary@mary.zone>

### displaydoc 0.2.7

<!-- cargo-package: displaydoc 0.2.7 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/yaahc/displaydoc
- **Homepage:** https://github.com/yaahc/displaydoc
- **Authors:** Jane Lusby <jlusby@yaah.dev>

### dlib 0.5.3

<!-- cargo-package: dlib 0.5.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/elinorbgr/dlib
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### document-features 0.2.12

<!-- cargo-package: document-features 0.2.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/slint-ui/document-features
- **Homepage:** https://slint.rs
- **Authors:** Slint Developers <info@slint.dev>

### downcast-rs 1.2.1

<!-- cargo-package: downcast-rs 1.2.1 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/marcianx/downcast-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Ashish Myles <marcianx@gmail.com>; Runji Wang <wangrunji0408@163.com>

### dpi 0.1.2

<!-- cargo-package: dpi 0.1.2 -->

- **License (Cargo metadata):** `Apache-2.0 AND MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/winit
- **Homepage:** Not specified in Cargo metadata

### ecolor 0.35.0

<!-- cargo-package: ecolor 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui
- **Homepage:** https://github.com/emilk/egui
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>; Andreas Reich <reichandreas@gmx.de>

### ed25519 2.2.3

<!-- cargo-package: ed25519 2.2.3 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/signatures/tree/master/ed25519
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### ed25519-dalek 2.2.0

<!-- cargo-package: ed25519-dalek 2.2.0 -->

- **License (Cargo metadata):** `BSD-3-Clause`
- **Repository:** https://github.com/dalek-cryptography/curve25519-dalek/tree/main/ed25519-dalek
- **Homepage:** https://github.com/dalek-cryptography/curve25519-dalek
- **Authors:** isis lovecruft <isis@patternsinthevoid.net>; Tony Arcieri <bascule@gmail.com>; Michael Rosenberg <michael@mrosenberg.pub>

### eframe 0.35.0

<!-- cargo-package: eframe 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui/tree/main/crates/eframe
- **Homepage:** https://github.com/emilk/egui/tree/main/crates/eframe
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### egui 0.35.0

<!-- cargo-package: egui 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui
- **Homepage:** https://github.com/emilk/egui
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### egui-wgpu 0.35.0

<!-- cargo-package: egui-wgpu 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui/tree/main/crates/egui-wgpu
- **Homepage:** https://github.com/emilk/egui/tree/main/crates/egui-wgpu
- **Authors:** Nils Hasenbanck <nils@hasenbanck.de>; embotech <opensource@embotech.com>; Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### egui-winit 0.35.0

<!-- cargo-package: egui-winit 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui/tree/main/crates/egui-winit
- **Homepage:** https://github.com/emilk/egui/tree/main/crates/egui-winit
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### egui_glow 0.35.0

<!-- cargo-package: egui_glow 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui/tree/main/crates/egui_glow
- **Homepage:** https://github.com/emilk/egui/tree/main/crates/egui_glow
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### egui_software_backend 0.0.2

<!-- cargo-package: egui_software_backend 0.0.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/DGriffin91/egui_software_backend
- **Homepage:** Not specified in Cargo metadata
- **Source:** Vendored path dependency at `vendor/egui_software_backend`
- **Vendored license texts:**
  - `vendor/egui_software_backend/LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)
  - `vendor/egui_software_backend/LICENSE-APACHE`: reproduced as [Appendix B](#appendix-b-apache-20-license)

### either 1.19.0

<!-- cargo-package: either 1.19.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rayon-rs/either
- **Homepage:** Not specified in Cargo metadata

### emath 0.35.0

<!-- cargo-package: emath 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui/tree/main/crates/emath
- **Homepage:** https://github.com/emilk/egui/tree/main/crates/emath
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### endi 1.1.1

<!-- cargo-package: endi 1.1.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/zeenix/endi
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Zeeshan Ali Khan <zeenix@gmail.com>

### enumflags2 0.7.12

<!-- cargo-package: enumflags2 0.7.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/meithecatte/enumflags2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** maik klein <maikklein@googlemail.com>; Maja Kądziołka <maya@compilercrim.es>

### enumflags2_derive 0.7.12

<!-- cargo-package: enumflags2_derive 0.7.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/meithecatte/enumflags2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** maik klein <maikklein@googlemail.com>; Maja Kądziołka <maya@compilercrim.es>

### epaint 0.35.0

<!-- cargo-package: epaint 0.35.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui/tree/main/crates/epaint
- **Homepage:** https://github.com/emilk/egui/tree/main/crates/epaint
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### epaint_default_fonts 0.35.0

<!-- cargo-package: epaint_default_fonts 0.35.0 -->

- **License (Cargo metadata):** `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts
- **Homepage:** https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts
- **Authors:** Emil Ernerfeldt <emil.ernerfeldt@gmail.com>

### equivalent 1.0.3

<!-- cargo-package: equivalent 1.0.3 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/indexmap-rs/equivalent
- **Homepage:** Not specified in Cargo metadata

### errno 0.3.14

<!-- cargo-package: errno 0.3.14 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/lambda-fairy/rust-errno
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Chris Wong <lambda.fairy@gmail.com>; Dan Gohman <dev@sunfishcode.online>

### error-code 3.4.0

<!-- cargo-package: error-code 3.4.0 -->

- **License (Cargo metadata):** `BSL-1.0`
- **Repository:** https://github.com/DoumanAsh/error-code
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Douman <douman@gmx.se>

### euclid 0.22.14

<!-- cargo-package: euclid 0.22.14 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/euclid
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Servo Project Developers

### evdev 0.13.2

<!-- cargo-package: evdev 0.13.2 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/cmr/evdev
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Corey Richardson <corey@octayn.net>

### event-listener 5.4.2

<!-- cargo-package: event-listener 5.4.2 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/event-listener
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Stjepan Glavina <stjepang@gmail.com>; John Nunley <dev@notgull.net>

### event-listener-strategy 0.5.4

<!-- cargo-package: event-listener-strategy 0.5.4 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/event-listener-strategy
- **Homepage:** Not specified in Cargo metadata
- **Authors:** John Nunley <dev@notgull.net>

### fastrand 2.5.0

<!-- cargo-package: fastrand 2.5.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/fastrand
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Stjepan Glavina <stjepang@gmail.com>

### fax 0.2.7

<!-- cargo-package: fax 0.2.7 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/pdf-rs/fax
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Sebastian K <s3bk@protonmail.com>

### fdeflate 0.3.7

<!-- cargo-package: fdeflate 0.3.7 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/image-rs/fdeflate
- **Homepage:** https://github.com/image-rs/fdeflate
- **Authors:** The image-rs Developers

### fearless_simd 0.4.1

<!-- cargo-package: fearless_simd 0.4.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/fearless_simd
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Raph Levien <raph.levien@gmail.com>

### fiat-crypto 0.2.9

<!-- cargo-package: fiat-crypto 0.2.9 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR BSD-1-Clause`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/mit-plv/fiat-crypto
- **Homepage:** https://github.com/mit-plv/fiat-crypto
- **Authors:** Fiat Crypto library authors <jgross@mit.edu>

### find-msvc-tools 0.1.15

<!-- cargo-package: find-msvc-tools 0.1.15 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/cc-rs
- **Homepage:** Not specified in Cargo metadata

### flate2 1.1.10

<!-- cargo-package: flate2 1.1.10 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/flate2-rs
- **Homepage:** https://github.com/rust-lang/flate2-rs
- **Authors:** Alex Crichton <alex@alexcrichton.com>; Josh Triplett <josh@joshtriplett.org>

### flume 0.12.0

<!-- cargo-package: flume 0.12.0 -->

- **License (Cargo metadata):** `Apache-2.0/MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/zesterer/flume
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Joshua Barretto <joshua.s.barretto@gmail.com>

### foldhash 0.2.0

<!-- cargo-package: foldhash 0.2.0 -->

- **License (Cargo metadata):** `Zlib`
- **Repository:** https://github.com/orlp/foldhash
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Orson Peters <orsonpeters@gmail.com>

### font-types 0.11.3

<!-- cargo-package: font-types 0.11.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/googlefonts/fontations
- **Homepage:** Not specified in Cargo metadata

### foreign-types 0.5.0

<!-- cargo-package: foreign-types 0.5.0 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sfackler/foreign-types
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Fackler <sfackler@gmail.com>

### foreign-types-macros 0.2.4

<!-- cargo-package: foreign-types-macros 0.2.4 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sfackler/foreign-types
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Fackler <sfackler@gmail.com>

### foreign-types-shared 0.3.1

<!-- cargo-package: foreign-types-shared 0.3.1 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sfackler/foreign-types
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Fackler <sfackler@gmail.com>

### form_urlencoded 1.2.2

<!-- cargo-package: form_urlencoded 1.2.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/rust-url
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The rust-url developers

### fs-set-times 0.20.3

<!-- cargo-package: fs-set-times 0.20.3 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/fs-set-times
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### funty 2.0.0

<!-- cargo-package: funty 2.0.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/myrrlyn/funty
- **Homepage:** Not specified in Cargo metadata
- **Authors:** myrrlyn <self@myrrlyn.dev>

### fuser 0.18.0

<!-- cargo-package: fuser 0.18.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/cberner/fuser
- **Homepage:** https://github.com/cberner/fuser
- **Authors:** Christopher Berner <me@cberner.com>
- **Vendored license texts:**
  - `assets/third-party/file-transfer/fuser-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### futures-channel 0.3.34

<!-- cargo-package: futures-channel 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/futures-rs
- **Homepage:** https://rust-lang.github.io/futures-rs

### futures-core 0.3.34

<!-- cargo-package: futures-core 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/futures-rs
- **Homepage:** https://rust-lang.github.io/futures-rs

### futures-io 0.3.34

<!-- cargo-package: futures-io 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/futures-rs
- **Homepage:** https://rust-lang.github.io/futures-rs

### futures-lite 2.6.1

<!-- cargo-package: futures-lite 2.6.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/futures-lite
- **Homepage:** https://github.com/smol-rs/futures-lite
- **Authors:** Stjepan Glavina <stjepang@gmail.com>; Contributors to futures-rs

### futures-macro 0.3.34

<!-- cargo-package: futures-macro 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/futures-rs
- **Homepage:** https://rust-lang.github.io/futures-rs

### futures-sink 0.3.34

<!-- cargo-package: futures-sink 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/futures-rs
- **Homepage:** https://rust-lang.github.io/futures-rs

### futures-task 0.3.34

<!-- cargo-package: futures-task 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/futures-rs
- **Homepage:** https://rust-lang.github.io/futures-rs

### futures-util 0.3.34

<!-- cargo-package: futures-util 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/futures-rs
- **Homepage:** https://rust-lang.github.io/futures-rs

### generic-array 0.14.7

<!-- cargo-package: generic-array 0.14.7 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/fizyk20/generic-array.git
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Bartłomiej Kamiński <fizyk20@gmail.com>; Aaron Trent <novacrazy@gmail.com>

### gethostname 1.1.0

<!-- cargo-package: gethostname 1.1.0 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://codeberg.org/swsnr/gethostname.rs.git
- **Homepage:** https://codeberg.org/swsnr/gethostname.rs
- **Authors:** Sebastian Wiesner <sebastian@swsnr.de>

### getrandom 0.2.17

<!-- cargo-package: getrandom 0.2.17 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-random/getrandom
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Rand Project Developers

### getrandom 0.3.4

<!-- cargo-package: getrandom 0.3.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-random/getrandom
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Rand Project Developers

### getrandom 0.4.3

<!-- cargo-package: getrandom 0.4.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-random/getrandom
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Rand Project Developers

### gl_generator 0.14.0

<!-- cargo-package: gl_generator 0.14.0 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/brendanzab/gl-rs/
- **Homepage:** https://github.com/brendanzab/gl-rs/
- **Authors:** Brendan Zabarauskas <bjzaba@yahoo.com.au>; Corey Richardson; Arseny Kapoulkine

### glifo 0.1.1

<!-- cargo-package: glifo 0.1.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/vello
- **Homepage:** Not specified in Cargo metadata

### glow 0.17.0

<!-- cargo-package: glow 0.17.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR Zlib`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/grovesNL/glow
- **Homepage:** https://github.com/grovesNL/glow.git
- **Authors:** Joshua Groves <josh@joshgroves.com>; Dzmitry Malyshau <kvarkus@gmail.com>

### glutin 0.32.3

<!-- cargo-package: glutin 0.32.3 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/glutin
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### glutin-winit 0.5.0

<!-- cargo-package: glutin-winit 0.5.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/rust-windowing/glutin
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### glutin_egl_sys 0.7.1

<!-- cargo-package: glutin_egl_sys 0.7.1 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/glutin
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### glutin_glx_sys 0.6.1

<!-- cargo-package: glutin_glx_sys 0.6.1 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/glutin
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### glutin_wgl_sys 0.6.1

<!-- cargo-package: glutin_wgl_sys 0.6.1 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/glutin
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### guillotiere 0.7.0

<!-- cargo-package: guillotiere 0.7.0 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/nical/guillotiere
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Nicolas Silva <nical@fastmail.com>

### half 2.7.1

<!-- cargo-package: half 2.7.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/VoidStarKat/half-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kathryn Long <squeeself@gmail.com>

### harfrust 0.7.0

<!-- cargo-package: harfrust 0.7.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/harfbuzz/harfrust
- **Homepage:** Not specified in Cargo metadata

### hashbrown 0.16.1

<!-- cargo-package: hashbrown 0.16.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/hashbrown
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Amanieu d'Antras <amanieu@gmail.com>

### hashbrown 0.17.1

<!-- cargo-package: hashbrown 0.17.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/hashbrown
- **Homepage:** Not specified in Cargo metadata

### hermit-abi 0.5.3

<!-- cargo-package: hermit-abi 0.5.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/hermit-os/hermit-rs
- **Homepage:** Not specified in Cargo metadata

### hex 0.4.3

<!-- cargo-package: hex 0.4.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/KokaKiwi/rust-hex
- **Homepage:** Not specified in Cargo metadata
- **Authors:** KokaKiwi <kokakiwi@kokakiwi.net>

### hexf-parse 0.2.1

<!-- cargo-package: hexf-parse 0.2.1 -->

- **License (Cargo metadata):** `CC0-1.0`
- **Repository:** https://github.com/lifthrasiir/hexf
- **Homepage:** https://github.com/lifthrasiir/hexf
- **Authors:** Kang Seonghoon <public+rust@mearie.org>

### http 1.5.0

<!-- cargo-package: http 1.5.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/hyperium/http
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Alex Crichton <alex@alexcrichton.com>; Carl Lerche <me@carllerche.com>; Sean McArthur <sean@seanmonstar.com>

### http-body 1.1.0

<!-- cargo-package: http-body 1.1.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/hyperium/http-body
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Carl Lerche <me@carllerche.com>; Lucio Franco <luciofranco14@gmail.com>; Sean McArthur <sean@seanmonstar.com>

### http-body-util 0.1.5

<!-- cargo-package: http-body-util 0.1.5 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/hyperium/http-body
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Carl Lerche <me@carllerche.com>; Lucio Franco <luciofranco14@gmail.com>; Sean McArthur <sean@seanmonstar.com>

### httparse 1.10.1

<!-- cargo-package: httparse 1.10.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/seanmonstar/httparse
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Sean McArthur <sean@seanmonstar.com>

### hyper 1.12.0

<!-- cargo-package: hyper 1.12.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/hyperium/hyper
- **Homepage:** https://hyper.rs
- **Authors:** Sean McArthur <sean@seanmonstar.com>

### hyper-rustls 0.27.10

<!-- cargo-package: hyper-rustls 0.27.10 -->

- **License (Cargo metadata):** `Apache-2.0 OR ISC OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rustls/hyper-rustls
- **Homepage:** https://github.com/rustls/hyper-rustls

### hyper-util 0.1.21

<!-- cargo-package: hyper-util 0.1.21 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/hyperium/hyper-util
- **Homepage:** https://hyper.rs
- **Authors:** Sean McArthur <sean@seanmonstar.com>

### icu_collections 2.4.0

<!-- cargo-package: icu_collections 2.4.0 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### icu_locale_core 2.4.0

<!-- cargo-package: icu_locale_core 2.4.0 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### icu_normalizer 2.4.0

<!-- cargo-package: icu_normalizer 2.4.0 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### icu_normalizer_data 2.4.0

<!-- cargo-package: icu_normalizer_data 2.4.0 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### icu_properties 2.4.0

<!-- cargo-package: icu_properties 2.4.0 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### icu_properties_data 2.4.0

<!-- cargo-package: icu_properties_data 2.4.0 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### icu_provider 2.4.0

<!-- cargo-package: icu_provider 2.4.0 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### idna 1.1.0

<!-- cargo-package: idna 1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/rust-url/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The rust-url developers

### idna_adapter 1.2.2

<!-- cargo-package: idna_adapter 1.2.2 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/hsivonen/idna_adapter
- **Homepage:** https://docs.rs/crate/idna_adapter/latest
- **Authors:** The rust-url developers

### if-addrs 0.15.0

<!-- cargo-package: if-addrs 0.15.0 -->

- **License (Cargo metadata):** `MIT OR BSD-3-Clause`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/messense/if-addrs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** MaidSafe Developers <dev@maidsafe.net>; Messense Lv <messense@icloud.com>

### image 0.25.10

<!-- cargo-package: image 0.25.10 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/image-rs/image
- **Homepage:** https://github.com/image-rs/image
- **Authors:** The image-rs Developers

### indexmap 2.14.2

<!-- cargo-package: indexmap 2.14.2 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/indexmap-rs/indexmap
- **Homepage:** Not specified in Cargo metadata

### inout 0.1.4

<!-- cargo-package: inout 0.1.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/utils
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### io-extras 0.19.0

<!-- cargo-package: io-extras 0.19.0 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/io-extras
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### io-lifetimes 2.0.4

<!-- cargo-package: io-lifetimes 2.0.4 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/io-lifetimes
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### io-lifetimes 3.0.1

<!-- cargo-package: io-lifetimes 3.0.1 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/io-lifetimes
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### ipnet 2.12.2

<!-- cargo-package: ipnet 2.12.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/krisprice/ipnet
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kris Price <kris@krisprice.nz>

### itertools 0.14.0

<!-- cargo-package: itertools 0.14.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-itertools/itertools
- **Homepage:** Not specified in Cargo metadata
- **Authors:** bluss

### itoa 1.0.18

<!-- cargo-package: itoa 1.0.18 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/itoa
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### jni 0.22.4

<!-- cargo-package: jni 0.22.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/jni-rs/jni-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** jni team

### jni-macros 0.22.4

<!-- cargo-package: jni-macros 0.22.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/jni-rs/jni-rs
- **Homepage:** Not specified in Cargo metadata

### jni-sys 0.3.1

<!-- cargo-package: jni-sys 0.3.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/jni-rs/jni-sys
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Fackler <sfackler@gmail.com>

### jni-sys 0.4.1

<!-- cargo-package: jni-sys 0.4.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/jni-rs/jni-sys
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Fackler <sfackler@gmail.com>; Robert Bragg <robert@sixbynine.org>

### jni-sys-macros 0.4.1

<!-- cargo-package: jni-sys-macros 0.4.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/jni-rs/jni-sys
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Robert Bragg <robert@sixbynine.org>

### jobserver 0.1.35

<!-- cargo-package: jobserver 0.1.35 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/jobserver-rs
- **Homepage:** https://github.com/rust-lang/jobserver-rs
- **Authors:** Alex Crichton <alex@alexcrichton.com>

### js-sys 0.3.107

<!-- cargo-package: js-sys 0.3.107 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/js-sys
- **Homepage:** https://wasm-bindgen.github.io/wasm-bindgen/
- **Authors:** The wasm-bindgen Developers

### keyboard-types 0.8.3

<!-- cargo-package: keyboard-types 0.8.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/keyboard-types
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Pyfisch <pyfisch@posteo.org>

### khronos_api 3.1.0

<!-- cargo-package: khronos_api 3.1.0 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/brendanzab/gl-rs/
- **Homepage:** https://github.com/brendanzab/gl-rs/
- **Authors:** Brendan Zabarauskas <bjzaba@yahoo.com.au>; Corey Richardson; Arseny Kapoulkine; Pierre Krieger <pierre.krieger1708@gmail.com>

### ksni 0.3.7

<!-- cargo-package: ksni 0.3.7 -->

- **License (Cargo metadata):** `Unlicense`
- **Repository:** https://github.com/iovxw/ksni
- **Homepage:** Not specified in Cargo metadata
- **Authors:** iovxw <iovxw@outlook.com>

### kurbo 0.13.1

<!-- cargo-package: kurbo 0.13.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/kurbo
- **Homepage:** Not specified in Cargo metadata

### libc 0.2.191

<!-- cargo-package: libc 0.2.191 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/libc
- **Homepage:** Not specified in Cargo metadata

### libloading 0.8.9

<!-- cargo-package: libloading 0.8.9 -->

- **License (Cargo metadata):** `ISC`
- **Repository:** https://github.com/nagisa/rust_libloading/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Simonas Kazlauskas <libloading@kazlauskas.me>

### libm 0.2.16

<!-- cargo-package: libm 0.2.16 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/rust-lang/compiler-builtins
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Alex Crichton <alex@alexcrichton.com>; Amanieu d'Antras <amanieu@gmail.com>; Jorge Aparicio <japaricious@gmail.com>; Trevor Gross <tg@trevorgross.com>

### libredox 0.1.25

<!-- cargo-package: libredox 0.1.25 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://gitlab.redox-os.org/redox-os/libredox.git
- **Homepage:** Not specified in Cargo metadata
- **Authors:** 4lDO2 <4lDO2@protonmail.com>

### linebender_resource_handle 0.1.1

<!-- cargo-package: linebender_resource_handle 0.1.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/raw_resource_handle
- **Homepage:** Not specified in Cargo metadata

### linux-raw-sys 0.12.1

<!-- cargo-package: linux-raw-sys 0.12.1 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/linux-raw-sys
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### linux-raw-sys 0.4.15

<!-- cargo-package: linux-raw-sys 0.4.15 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/linux-raw-sys
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### litemap 0.8.3

<!-- cargo-package: litemap 0.8.3 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The ICU4X Project Developers

### litrs 1.0.1

<!-- cargo-package: litrs 1.0.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/LukasKalbertodt/litrs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Lukas Kalbertodt <lukas.kalbertodt@gmail.com>

### lock_api 0.4.14

<!-- cargo-package: lock_api 0.4.14 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Amanieu/parking_lot
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Amanieu d'Antras <amanieu@gmail.com>

### log 0.4.35

<!-- cargo-package: log 0.4.35 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/log
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Rust Project Developers

### lru-slab 0.1.3

<!-- cargo-package: lru-slab 0.1.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR Zlib`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Ralith/lru-slab
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Benjamin Saunders <ben.e.saunders@gmail.com>

### maybe-owned 0.3.4

<!-- cargo-package: maybe-owned 0.3.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rustonaut/maybe-owned
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Philipp korber <philippkorber@gmail.com>; darkstalker <slayerbeast@gmail.com>; daboross <daboross@daboross.net>; Zac Burns (That3Percent) <that3percent@gmail.com>

### mdns-sd 0.21.4

<!-- cargo-package: mdns-sd 0.21.4 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/keepsimple1/mdns-sd
- **Homepage:** Not specified in Cargo metadata
- **Authors:** keepsimple <keepsimple@gmail.com>

### memchr 2.8.3

<!-- cargo-package: memchr 2.8.3 -->

- **License (Cargo metadata):** `Unlicense OR MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/BurntSushi/memchr
- **Homepage:** https://github.com/BurntSushi/memchr
- **Authors:** Andrew Gallant <jamslam@gmail.com>; bluss

### memmap2 0.9.12

<!-- cargo-package: memmap2 0.9.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RazrFalcon/memmap2-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Burkert <dan@danburkert.com>; Yevhenii Reizner <razrfalcon@gmail.com>; The Contributors

### memoffset 0.9.1

<!-- cargo-package: memoffset 0.9.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/Gilnaa/memoffset
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Gilad Naaman <gilad.naaman@gmail.com>

### miniz_oxide 0.8.9

<!-- cargo-package: miniz_oxide 0.8.9 -->

- **License (Cargo metadata):** `MIT OR Zlib OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide
- **Homepage:** https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide
- **Authors:** Frommi <daniil.liferenko@gmail.com>; oyvindln <oyvindln@users.noreply.github.com>; Rich Geldreich richgel99@gmail.com

### miniz_oxide 0.9.1

<!-- cargo-package: miniz_oxide 0.9.1 -->

- **License (Cargo metadata):** `MIT OR Zlib OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide
- **Homepage:** https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide
- **Authors:** Frommi <daniil.liferenko@gmail.com>; oyvindln <oyvindln@users.noreply.github.com>; Rich Geldreich richgel99@gmail.com

### mio 1.2.4

<!-- cargo-package: mio 1.2.4 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/mio
- **Homepage:** https://github.com/tokio-rs/mio
- **Authors:** Carl Lerche <me@carllerche.com>; Thomas de Zeeuw <thomasdezeeuw@gmail.com>; Tokio Contributors <team@tokio.rs>

### moxcms 0.8.1

<!-- cargo-package: moxcms 0.8.1 -->

- **License (Cargo metadata):** `BSD-3-Clause OR Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/awxkee/moxcms.git
- **Homepage:** https://github.com/awxkee/moxcms
- **Authors:** Radzivon Bartoshyk

### muda 0.20.0

<!-- cargo-package: muda 0.20.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/tauri-apps/muda
- **Homepage:** Not specified in Cargo metadata

### naga 29.0.4

<!-- cargo-package: naga 29.0.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/gfx-rs/wgpu
- **Homepage:** Not specified in Cargo metadata
- **Authors:** gfx-rs developers

### ndk 0.9.0

<!-- cargo-package: ndk 0.9.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-mobile/ndk
- **Homepage:** https://github.com/rust-mobile/ndk
- **Authors:** The Rust Mobile contributors

### ndk-context 0.1.1

<!-- cargo-package: ndk-context 0.1.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/android-ndk-rs
- **Homepage:** https://github.com/rust-windowing/android-ndk-rs
- **Authors:** The Rust Windowing contributors

### ndk-sys 0.6.0+11769913

<!-- cargo-package: ndk-sys 0.6.0+11769913 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-mobile/ndk
- **Homepage:** https://github.com/rust-mobile/ndk
- **Authors:** The Rust Windowing contributors

### nix 0.29.0

<!-- cargo-package: nix 0.29.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/nix-rust/nix
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The nix-rust Project Developers
- **Vendored license texts:**
  - `assets/third-party/file-transfer/nix-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### nix 0.31.3

<!-- cargo-package: nix 0.31.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/nix-rust/nix
- **Homepage:** Not specified in Cargo metadata
- **Vendored license texts:**
  - `assets/third-party/file-transfer/nix-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### nohash-hasher 0.2.0

<!-- cargo-package: nohash-hasher 0.2.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/paritytech/nohash-hasher
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Parity Technologies <admin@parity.io>

### num-integer 0.1.47

<!-- cargo-package: num-integer 0.1.47 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-num/num-integer
- **Homepage:** https://github.com/rust-num/num-integer
- **Authors:** The Rust Project Developers

### num-traits 0.2.19

<!-- cargo-package: num-traits 0.2.19 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-num/num-traits
- **Homepage:** https://github.com/rust-num/num-traits
- **Authors:** The Rust Project Developers

### num_enum 0.7.6

<!-- cargo-package: num_enum 0.7.6 -->

- **License (Cargo metadata):** `BSD-3-Clause OR MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/illicitonion/num_enum
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Daniel Wagner-Hall <dawagner@gmail.com>; Daniel Henry-Mantilla <daniel.henry.mantilla@gmail.com>; Vincent Esche <regexident@gmail.com>

### num_enum_derive 0.7.6

<!-- cargo-package: num_enum_derive 0.7.6 -->

- **License (Cargo metadata):** `BSD-3-Clause OR MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/illicitonion/num_enum
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Daniel Wagner-Hall <dawagner@gmail.com>; Daniel Henry-Mantilla <daniel.henry.mantilla@gmail.com>; Vincent Esche <regexident@gmail.com>

### objc-sys 0.3.5

<!-- cargo-package: objc-sys 0.3.5 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Mads Marquart <mads@marquart.dk>

### objc2 0.5.3

<!-- cargo-package: objc2 0.5.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Steven Sheldon; Mads Marquart <mads@marquart.dk>

### objc2 0.6.5

<!-- cargo-package: objc2 0.6.5 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Mads Marquart <mads@marquart.dk>

### objc2-app-kit 0.2.2

<!-- cargo-package: objc2-app-kit 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-app-kit 0.3.2

<!-- cargo-package: objc2-app-kit 0.3.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-cloud-kit 0.2.2

<!-- cargo-package: objc2-cloud-kit 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-contacts 0.2.2

<!-- cargo-package: objc2-contacts 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-core-data 0.2.2

<!-- cargo-package: objc2-core-data 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-core-foundation 0.3.2

<!-- cargo-package: objc2-core-foundation 0.3.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-core-graphics 0.3.2

<!-- cargo-package: objc2-core-graphics 0.3.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-core-image 0.2.2

<!-- cargo-package: objc2-core-image 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-core-location 0.2.2

<!-- cargo-package: objc2-core-location 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-encode 4.1.0

<!-- cargo-package: objc2-encode 4.1.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Mads Marquart <mads@marquart.dk>

### objc2-foundation 0.2.2

<!-- cargo-package: objc2-foundation 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-foundation 0.3.2

<!-- cargo-package: objc2-foundation 0.3.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-io-surface 0.3.2

<!-- cargo-package: objc2-io-surface 0.3.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-link-presentation 0.2.2

<!-- cargo-package: objc2-link-presentation 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-metal 0.2.2

<!-- cargo-package: objc2-metal 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-quartz-core 0.2.2

<!-- cargo-package: objc2-quartz-core 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-quartz-core 0.3.2

<!-- cargo-package: objc2-quartz-core 0.3.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-symbols 0.2.2

<!-- cargo-package: objc2-symbols 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-ui-kit 0.2.2

<!-- cargo-package: objc2-ui-kit 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-ui-kit 0.3.2

<!-- cargo-package: objc2-ui-kit 0.3.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-uniform-type-identifiers 0.2.2

<!-- cargo-package: objc2-uniform-type-identifiers 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-uniform-type-identifiers 0.3.2

<!-- cargo-package: objc2-uniform-type-identifiers 0.3.2 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### objc2-user-notifications 0.2.2

<!-- cargo-package: objc2-user-notifications 0.2.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/madsmtm/objc2
- **Homepage:** Not specified in Cargo metadata

### once_cell 1.21.4

<!-- cargo-package: once_cell 1.21.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/matklad/once_cell
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Aleksey Kladov <aleksey.kladov@gmail.com>

### opaque-debug 0.3.1

<!-- cargo-package: opaque-debug 0.3.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/utils
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### option-ext 0.2.0

<!-- cargo-package: option-ext 0.2.0 -->

- **License (Cargo metadata):** `MPL-2.0`
- **Repository:** https://github.com/soc/option-ext.git
- **Homepage:** https://github.com/soc/option-ext
- **Authors:** Simon Ochsenreither <simon@ochsenreither.de>

### orbclient 0.3.55

<!-- cargo-package: orbclient 0.3.55 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://gitlab.redox-os.org/redox-os/orbclient
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jeremy Soller <jackpot51@gmail.com>

### ordered-stream 0.2.0

<!-- cargo-package: ordered-stream 0.2.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/danieldg/ordered-stream
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Daniel De Graaf <code@danieldg.net>; Zeeshan Ali Khan <zeeshanak@gnome.org>

### page_size 0.6.0

<!-- cargo-package: page_size 0.6.0 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Elzair/page_size_rs
- **Homepage:** https://github.com/Elzair/page_size_rs
- **Authors:** Philip Woods <elzairthesorcerer@gmail.com>
- **Vendored license texts:**
  - `assets/third-party/file-transfer/page-size-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### parking 2.2.1

<!-- cargo-package: parking 2.2.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/parking
- **Homepage:** https://github.com/smol-rs/parking
- **Authors:** Stjepan Glavina <stjepang@gmail.com>; The Rust Project Developers

### parking_lot 0.12.5

<!-- cargo-package: parking_lot 0.12.5 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Amanieu/parking_lot
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Amanieu d'Antras <amanieu@gmail.com>

### parking_lot_core 0.9.12

<!-- cargo-package: parking_lot_core 0.9.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Amanieu/parking_lot
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Amanieu d'Antras <amanieu@gmail.com>

### pastey 0.2.3

<!-- cargo-package: pastey 0.2.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/as1100k/pastey
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Aditya Kumar <git@adityais.dev>; David Tolnay <dtolnay@gmail.com>

### peniko 0.6.1

<!-- cargo-package: peniko 0.6.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/peniko
- **Homepage:** Not specified in Cargo metadata

### percent-encoding 2.3.2

<!-- cargo-package: percent-encoding 2.3.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/rust-url/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The rust-url developers

### pin-project 1.1.13

<!-- cargo-package: pin-project 1.1.13 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/taiki-e/pin-project
- **Homepage:** Not specified in Cargo metadata

### pin-project-internal 1.1.13

<!-- cargo-package: pin-project-internal 1.1.13 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/taiki-e/pin-project
- **Homepage:** Not specified in Cargo metadata

### pin-project-lite 0.2.17

<!-- cargo-package: pin-project-lite 0.2.17 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/taiki-e/pin-project-lite
- **Homepage:** Not specified in Cargo metadata

### pkcs8 0.10.2

<!-- cargo-package: pkcs8 0.10.2 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/formats/tree/master/pkcs8
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### pkg-config 0.3.34

<!-- cargo-package: pkg-config 0.3.34 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/pkg-config-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Alex Crichton <alex@alexcrichton.com>

### plain 0.2.3

<!-- cargo-package: plain 0.2.3 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/randomites/plain
- **Homepage:** https://github.com/randomites/plain
- **Authors:** jzr

### png 0.18.1

<!-- cargo-package: png 0.18.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/image-rs/image-png
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The image-rs Developers

### polling 3.11.0

<!-- cargo-package: polling 3.11.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/smol-rs/polling
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Stjepan Glavina <stjepang@gmail.com>; John Nunley <dev@notgull.net>

### pollster 1.0.1

<!-- cargo-package: pollster 1.0.1 -->

- **License (Cargo metadata):** `Apache-2.0/MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/zesterer/pollster
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Joshua Barretto <joshua@jsbarretto.com>

### poly1305 0.8.0

<!-- cargo-package: poly1305 0.8.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/universal-hashes
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### polycool 0.4.0

<!-- cargo-package: polycool 0.4.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/kurbo
- **Homepage:** Not specified in Cargo metadata

### portable-atomic 1.15.0

<!-- cargo-package: portable-atomic 1.15.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/taiki-e/portable-atomic
- **Homepage:** Not specified in Cargo metadata

### portable-atomic-util 0.2.8

<!-- cargo-package: portable-atomic-util 0.2.8 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/taiki-e/portable-atomic-util
- **Homepage:** Not specified in Cargo metadata

### potential_utf 0.1.6

<!-- cargo-package: potential_utf 0.1.6 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### proc-macro-crate 3.5.0

<!-- cargo-package: proc-macro-crate 3.5.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bkchr/proc-macro-crate
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Bastian Köcher <git@kchr.de>

### proc-macro2 1.0.107

<!-- cargo-package: proc-macro2 1.0.107 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/proc-macro2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>; Alex Crichton <alex@alexcrichton.com>

### profiling 1.0.18

<!-- cargo-package: profiling 1.0.18 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/aclysma/profiling
- **Homepage:** https://github.com/aclysma/profiling
- **Authors:** Philip Degarmo <aclysma@gmail.com>

### pxfm 0.1.30

<!-- cargo-package: pxfm 0.1.30 -->

- **License (Cargo metadata):** `BSD-3-Clause OR Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/awxkee/pxfm
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Radzivon Bartoshyk

### quick-error 2.0.1

<!-- cargo-package: quick-error 2.0.1 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** http://github.com/tailhook/quick-error
- **Homepage:** http://github.com/tailhook/quick-error
- **Authors:** Paul Colomiets <paul@colomiets.name>; Colin Kiegel <kiegel@gmx.de>

### quick-xml 0.41.0

<!-- cargo-package: quick-xml 0.41.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tafia/quick-xml
- **Homepage:** Not specified in Cargo metadata

### quinn 0.11.12

<!-- cargo-package: quinn 0.11.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/quinn-rs/quinn
- **Homepage:** Not specified in Cargo metadata

### quinn-proto 0.11.19

<!-- cargo-package: quinn-proto 0.11.19 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/quinn-rs/quinn
- **Homepage:** Not specified in Cargo metadata

### quinn-udp 0.5.16

<!-- cargo-package: quinn-udp 0.5.16 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/quinn-rs/quinn
- **Homepage:** Not specified in Cargo metadata

### quote 1.0.47

<!-- cargo-package: quote 1.0.47 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/quote
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### r-efi 5.3.0

<!-- cargo-package: r-efi 5.3.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR LGPL-2.1-or-later`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/r-efi/r-efi
- **Homepage:** https://github.com/r-efi/r-efi/wiki

### r-efi 6.0.0

<!-- cargo-package: r-efi 6.0.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR LGPL-2.1-or-later`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/r-efi/r-efi
- **Homepage:** https://github.com/r-efi/r-efi/wiki

### radium 0.7.0

<!-- cargo-package: radium 0.7.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/bitvecto-rs/radium
- **Homepage:** https://github.com/bitvecto-rs/radium
- **Authors:** Nika Layzell <nika@thelayzells.com>; myrrlyn <self@myrrlyn.dev>

### rand 0.10.3

<!-- cargo-package: rand 0.10.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-random/rand
- **Homepage:** https://rust-random.github.io/book
- **Authors:** The Rand Project Developers; The Rust Project Developers

### rand_core 0.10.1

<!-- cargo-package: rand_core 0.10.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-random/rand_core
- **Homepage:** https://rust-random.github.io/book
- **Authors:** The Rand Project Developers

### rand_core 0.6.4

<!-- cargo-package: rand_core 0.6.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-random/rand
- **Homepage:** https://rust-random.github.io/book
- **Authors:** The Rand Project Developers; The Rust Project Developers

### rand_pcg 0.10.2

<!-- cargo-package: rand_pcg 0.10.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-random/rngs
- **Homepage:** https://rust-random.github.io/book
- **Authors:** The Rand Project Developers

### raw-window-handle 0.6.2

<!-- cargo-package: raw-window-handle 0.6.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR Zlib`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/raw-window-handle
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Osspial <osspial@gmail.com>

### read-fonts 0.39.2

<!-- cargo-package: read-fonts 0.39.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/googlefonts/fontations
- **Homepage:** Not specified in Cargo metadata

### redox_syscall 0.4.1

<!-- cargo-package: redox_syscall 0.4.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://gitlab.redox-os.org/redox-os/syscall
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jeremy Soller <jackpot51@gmail.com>

### redox_syscall 0.5.18

<!-- cargo-package: redox_syscall 0.5.18 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://gitlab.redox-os.org/redox-os/syscall
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jeremy Soller <jackpot51@gmail.com>

### redox_syscall 0.9.5

<!-- cargo-package: redox_syscall 0.9.5 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://gitlab.redox-os.org/redox-os/kernel
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jeremy Soller <jackpot51@gmail.com>

### redox_users 0.5.3

<!-- cargo-package: redox_users 0.5.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://gitlab.redox-os.org/redox-os/users
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jose Narvaez <goyox86@gmail.com>; Wesley Hershberger <mggmugginsmc@gmail.com>

### ref-cast 1.0.27

<!-- cargo-package: ref-cast 1.0.27 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/ref-cast
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>
- **Vendored license texts:**
  - `assets/third-party/file-transfer/ref-cast-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### ref-cast-impl 1.0.27

<!-- cargo-package: ref-cast-impl 1.0.27 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/ref-cast
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>
- **Vendored license texts:**
  - `assets/third-party/file-transfer/ref-cast-LICENSE-MIT`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### reis 0.7.1

<!-- cargo-package: reis 0.7.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/ids1024/reis
- **Homepage:** Not specified in Cargo metadata

### renderdoc-sys 1.1.0

<!-- cargo-package: renderdoc-sys 1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/ebkalderon/renderdoc-rs
- **Homepage:** https://github.com/ebkalderon/renderdoc-rs/tree/master/renderdoc-sys
- **Authors:** Eyal Kalderon <ebkalderon@gmail.com>

### reqwest 0.12.28

<!-- cargo-package: reqwest 0.12.28 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/seanmonstar/reqwest
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Sean McArthur <sean@seanmonstar.com>

### rfd 0.18.0

<!-- cargo-package: rfd 0.18.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/PolyMeilex/rfd
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Poly <marynczak.bartlomiej@gmail.com>
- **Vendored license texts:**
  - `assets/third-party/file-transfer/rfd-LICENSE`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)
  - `assets/third-party/file-transfer/rfd-window-identifier-LICENSE`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### ring 0.17.14

<!-- cargo-package: ring 0.17.14 -->

- **License (Cargo metadata):** `Apache-2.0 AND ISC`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/briansmith/ring
- **Homepage:** Not specified in Cargo metadata

### rust-auto-updator 0.4.1

<!-- cargo-package: rust-auto-updator 0.4.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/siarheisinelnikau/rust-auto-updator
- **Homepage:** Not specified in Cargo metadata
- **Source:** Vendored path dependency at `vendor/rust-auto-updator`
- **Vendored license texts:**
  - `vendor/rust-auto-updator/LICENSE`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)

### rustc-hash 1.1.0

<!-- cargo-package: rustc-hash 1.1.0 -->

- **License (Cargo metadata):** `Apache-2.0/MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang-nursery/rustc-hash
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Rust Project Developers

### rustc-hash 2.1.3

<!-- cargo-package: rustc-hash 2.1.3 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/rustc-hash
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Rust Project Developers

### rustc_version 0.4.1

<!-- cargo-package: rustc_version 0.4.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/djc/rustc-version-rs
- **Homepage:** Not specified in Cargo metadata

### rustix 0.38.44

<!-- cargo-package: rustix 0.38.44 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/rustix
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>; Jakub Konka <kubkon@jakubkonka.com>

### rustix 1.1.6

<!-- cargo-package: rustix 1.1.6 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/rustix
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>; Jakub Konka <kubkon@jakubkonka.com>

### rustix-linux-procfs 0.1.1

<!-- cargo-package: rustix-linux-procfs 0.1.1 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/rustix-linux-procfs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Dan Gohman <dev@sunfishcode.online>

### rustls 0.23.46

<!-- cargo-package: rustls 0.23.46 -->

- **License (Cargo metadata):** `Apache-2.0 OR ISC OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rustls/rustls
- **Homepage:** https://github.com/rustls/rustls

### rustls-pki-types 1.15.1

<!-- cargo-package: rustls-pki-types 1.15.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rustls/pki-types
- **Homepage:** https://github.com/rustls/pki-types

### rustls-webpki 0.103.16

<!-- cargo-package: rustls-webpki 0.103.16 -->

- **License (Cargo metadata):** `ISC`
- **Repository:** https://github.com/rustls/webpki
- **Homepage:** Not specified in Cargo metadata

### rustversion 1.0.23

<!-- cargo-package: rustversion 1.0.23 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/rustversion
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### ryu 1.0.23

<!-- cargo-package: ryu 1.0.23 -->

- **License (Cargo metadata):** `Apache-2.0 OR BSL-1.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/ryu
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### same-file 1.0.6

<!-- cargo-package: same-file 1.0.6 -->

- **License (Cargo metadata):** `Unlicense/MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/BurntSushi/same-file
- **Homepage:** https://github.com/BurntSushi/same-file
- **Authors:** Andrew Gallant <jamslam@gmail.com>

### scoped-tls 1.0.1

<!-- cargo-package: scoped-tls 1.0.1 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/alexcrichton/scoped-tls
- **Homepage:** https://github.com/alexcrichton/scoped-tls
- **Authors:** Alex Crichton <alex@alexcrichton.com>

### scopeguard 1.2.0

<!-- cargo-package: scopeguard 1.2.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bluss/scopeguard
- **Homepage:** Not specified in Cargo metadata
- **Authors:** bluss

### self_cell 1.3.0

<!-- cargo-package: self_cell 1.3.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR GPL-2.0-only`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Voultapher/self_cell
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Lukas Bergdoll <lukas.bergdoll@gmail.com>

### semver 1.0.28

<!-- cargo-package: semver 1.0.28 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/semver
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### serde 1.0.229

<!-- cargo-package: serde 1.0.229 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/serde-rs/serde
- **Homepage:** https://serde.rs
- **Authors:** Erick Tryzelaar <erick.tryzelaar@gmail.com>; David Tolnay <dtolnay@gmail.com>

### serde_core 1.0.229

<!-- cargo-package: serde_core 1.0.229 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/serde-rs/serde
- **Homepage:** https://serde.rs
- **Authors:** Erick Tryzelaar <erick.tryzelaar@gmail.com>; David Tolnay <dtolnay@gmail.com>

### serde_derive 1.0.229

<!-- cargo-package: serde_derive 1.0.229 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/serde-rs/serde
- **Homepage:** https://serde.rs
- **Authors:** Erick Tryzelaar <erick.tryzelaar@gmail.com>; David Tolnay <dtolnay@gmail.com>

### serde_json 1.0.154

<!-- cargo-package: serde_json 1.0.154 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/serde-rs/json
- **Homepage:** Not specified in Cargo metadata

### serde_repr 0.1.21

<!-- cargo-package: serde_repr 0.1.21 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/serde-repr
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### serde_spanned 1.1.2

<!-- cargo-package: serde_spanned 1.1.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/toml-rs/toml
- **Homepage:** Not specified in Cargo metadata

### serde_urlencoded 0.7.1

<!-- cargo-package: serde_urlencoded 0.7.1 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/nox/serde_urlencoded
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Anthony Ramine <n.oxyde@gmail.com>

### sha2 0.10.9

<!-- cargo-package: sha2 0.10.9 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/hashes
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### shlex 2.0.1

<!-- cargo-package: shlex 2.0.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/comex/rust-shlex
- **Homepage:** Not specified in Cargo metadata
- **Authors:** comex <comexk@gmail.com>; Fenhl <fenhl@fenhl.net>; Adrian Taylor <adetaylor@chromium.org>; Alex Touchet <alextouchet@outlook.com>; Daniel Parks <dp+git@oxidized.org>; Garrett Berg <googberg@gmail.com>

### signal-hook-registry 1.4.8

<!-- cargo-package: signal-hook-registry 1.4.8 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/vorner/signal-hook
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Michal 'vorner' Vaner <vorner@vorner.cz>; Masaki Hara <ackie.h.gmai@gmail.com>

### signature 2.2.0

<!-- cargo-package: signature 2.2.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/traits/tree/master/signature
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### simd-adler32 0.3.10

<!-- cargo-package: simd-adler32 0.3.10 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/mcountryman/simd-adler32
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Marvin Countryman <me@maar.vin>

### simd_cesu8 1.2.0

<!-- cargo-package: simd_cesu8 1.2.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/seancroach/simd_cesu8
- **Homepage:** https://github.com/seancroach/simd_cesu8
- **Authors:** Sean C. Roach <me@seancroach.dev>

### simdutf8 0.1.5

<!-- cargo-package: simdutf8 0.1.5 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rusticstuff/simdutf8
- **Homepage:** https://github.com/rusticstuff/simdutf8
- **Authors:** Hans Kratz <hans@appfour.com>

### skrifa 0.42.1

<!-- cargo-package: skrifa 0.42.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/googlefonts/fontations
- **Homepage:** Not specified in Cargo metadata

### slab 0.4.12

<!-- cargo-package: slab 0.4.12 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/slab
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Carl Lerche <me@carllerche.com>

### slotmap 1.1.1

<!-- cargo-package: slotmap 1.1.1 -->

- **License (Cargo metadata):** `Zlib`
- **Repository:** https://github.com/orlp/slotmap
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Orson Peters <orsonpeters@gmail.com>

### smallvec 1.16.4

<!-- cargo-package: smallvec 1.16.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/rust-smallvec
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Servo Project Developers

### smithay-client-toolkit 0.19.2

<!-- cargo-package: smithay-client-toolkit 0.19.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/client-toolkit
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>; i509VCB <mail@i509.me>; Ashley Wulber <ashley@system76.com>

### smithay-client-toolkit 0.20.0

<!-- cargo-package: smithay-client-toolkit 0.20.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/client-toolkit
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>; i509VCB <mail@i509.me>; Ashley Wulber <ashley@system76.com>

### smithay-clipboard 0.7.3

<!-- cargo-package: smithay-clipboard 0.7.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/smithay-clipboard
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>; Elinor Berger <elinor@safaradeg.net>

### smol_str 0.2.2

<!-- cargo-package: smol_str 0.2.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-analyzer/smol_str
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Aleksey Kladov <aleksey.kladov@gmail.com>

### snow 0.10.0

<!-- cargo-package: snow 0.10.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/mcginty/snow
- **Homepage:** https://github.com/mcginty/snow
- **Authors:** Jake McGinty <me@jakebot.org>; trevp

### socket-pktinfo 0.4.1

<!-- cargo-package: socket-pktinfo 0.4.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/pixsper/socket-pktinfo
- **Homepage:** https://github.com/pixsper/socket-pktinfo
- **Authors:** pixsperdavid <david@pixsper.com>

### socket2 0.6.5

<!-- cargo-package: socket2 0.6.5 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-lang/socket2
- **Homepage:** https://github.com/rust-lang/socket2
- **Authors:** Alex Crichton <alex@alexcrichton.com>; Thomas de Zeeuw <thomasdezeeuw@gmail.com>

### softbuffer 0.4.8

<!-- cargo-package: softbuffer 0.4.8 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/softbuffer
- **Homepage:** Not specified in Cargo metadata

### spin 0.9.9

<!-- cargo-package: spin 0.9.9 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/mvdnes/spin-rs.git
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Mathijs van de Nes <git@mathijs.vd-nes.nl>; John Ericson <git@JohnEricson.me>; Joshua Barretto <joshua.s.barretto@gmail.com>

### spki 0.7.3

<!-- cargo-package: spki 0.7.3 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/formats/tree/master/spki
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### stable_deref_trait 1.2.1

<!-- cargo-package: stable_deref_trait 1.2.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/storyyeller/stable_deref_trait
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Robert Grosse <n210241048576@gmail.com>

### static_assertions 1.1.0

<!-- cargo-package: static_assertions 1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/nvzqz/static-assertions-rs
- **Homepage:** https://github.com/nvzqz/static-assertions-rs
- **Authors:** Nikolai Vazquez

### strength_reduce 0.2.4

<!-- cargo-package: strength_reduce 0.2.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** http://github.com/ejmahler/strength_reduce
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elliott Mahler <join.together@gmail.com>

### subtle 2.6.1

<!-- cargo-package: subtle 2.6.1 -->

- **License (Cargo metadata):** `BSD-3-Clause`
- **Repository:** https://github.com/dalek-cryptography/subtle
- **Homepage:** https://dalek.rs/
- **Authors:** Isis Lovecruft <isis@patternsinthevoid.net>; Henry de Valence <hdevalence@hdevalence.ca>

### syn 2.0.119

<!-- cargo-package: syn 2.0.119 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/syn
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### syn 3.0.9

<!-- cargo-package: syn 3.0.9 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/syn
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### sync_wrapper 1.0.2

<!-- cargo-package: sync_wrapper 1.0.2 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Actyx/sync_wrapper
- **Homepage:** https://docs.rs/sync_wrapper
- **Authors:** Actyx AG <developer@actyx.io>

### synstructure 0.14.0

<!-- cargo-package: synstructure 0.14.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/mystor/synstructure
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Nika Layzell <nika@thelayzells.com>

### tap 1.0.1

<!-- cargo-package: tap 1.0.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/myrrlyn/tap
- **Homepage:** https://github.com/myrrlyn/tap
- **Authors:** Elliott Linder <elliott.darfink@gmail.com>; myrrlyn <self@myrrlyn.dev>

### tempfile 3.27.0

<!-- cargo-package: tempfile 3.27.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Stebalien/tempfile
- **Homepage:** https://stebalien.com/projects/tempfile-rs/
- **Authors:** Steven Allen <steven@stebalien.com>; The Rust Project Developers; Ashley Mannix <ashleymannix@live.com.au>; Jason White <me@jasonwhite.io>

### thiserror 1.0.69

<!-- cargo-package: thiserror 1.0.69 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/thiserror
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### thiserror 2.0.21

<!-- cargo-package: thiserror 2.0.21 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/thiserror
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### thiserror-impl 1.0.69

<!-- cargo-package: thiserror-impl 1.0.69 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/thiserror
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### thiserror-impl 2.0.21

<!-- cargo-package: thiserror-impl 2.0.21 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/thiserror
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### tiff 0.11.3

<!-- cargo-package: tiff 0.11.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/image-rs/image-tiff
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The image-rs Developers

### tinystr 0.8.5

<!-- cargo-package: tinystr 0.8.5 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The ICU4X Project Developers

### tinyvec 1.13.3

<!-- cargo-package: tinyvec 1.13.3 -->

- **License (Cargo metadata):** `Zlib OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/Lokathor/tinyvec
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Lokathor <zefria@gmail.com>

### tokio 1.54.1

<!-- cargo-package: tokio 1.54.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/tokio
- **Homepage:** https://tokio.rs
- **Authors:** Tokio Contributors <team@tokio.rs>

### tokio-macros 2.7.2

<!-- cargo-package: tokio-macros 2.7.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/tokio
- **Homepage:** https://tokio.rs
- **Authors:** Tokio Contributors <team@tokio.rs>

### tokio-rustls 0.26.6

<!-- cargo-package: tokio-rustls 0.26.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rustls/tokio-rustls
- **Homepage:** https://github.com/rustls/tokio-rustls

### toml 0.9.12+spec-1.1.0

<!-- cargo-package: toml 0.9.12+spec-1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/toml-rs/toml
- **Homepage:** Not specified in Cargo metadata

### toml_datetime 0.7.5+spec-1.1.0

<!-- cargo-package: toml_datetime 0.7.5+spec-1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/toml-rs/toml
- **Homepage:** Not specified in Cargo metadata

### toml_datetime 1.1.2+spec-1.1.0

<!-- cargo-package: toml_datetime 1.1.2+spec-1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/toml-rs/toml
- **Homepage:** Not specified in Cargo metadata

### toml_edit 0.25.18+spec-1.1.0

<!-- cargo-package: toml_edit 0.25.18+spec-1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/toml-rs/toml
- **Homepage:** Not specified in Cargo metadata

### toml_parser 1.1.5+spec-1.1.0

<!-- cargo-package: toml_parser 1.1.5+spec-1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/toml-rs/toml
- **Homepage:** Not specified in Cargo metadata

### toml_writer 1.2.0+spec-1.1.0

<!-- cargo-package: toml_writer 1.2.0+spec-1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/toml-rs/toml
- **Homepage:** Not specified in Cargo metadata

### tower 0.5.3

<!-- cargo-package: tower 0.5.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tower-rs/tower
- **Homepage:** https://github.com/tower-rs/tower
- **Authors:** Tower Maintainers <team@tower-rs.com>

### tower-http 0.6.11

<!-- cargo-package: tower-http 0.6.11 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tower-rs/tower-http
- **Homepage:** https://github.com/tower-rs/tower-http
- **Authors:** Tower Maintainers <team@tower-rs.com>

### tower-layer 0.3.3

<!-- cargo-package: tower-layer 0.3.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tower-rs/tower
- **Homepage:** https://github.com/tower-rs/tower
- **Authors:** Tower Maintainers <team@tower-rs.com>

### tower-service 0.3.3

<!-- cargo-package: tower-service 0.3.3 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tower-rs/tower
- **Homepage:** https://github.com/tower-rs/tower
- **Authors:** Tower Maintainers <team@tower-rs.com>

### tracing 0.1.44

<!-- cargo-package: tracing 0.1.44 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/tracing
- **Homepage:** https://tokio.rs
- **Authors:** Eliza Weisman <eliza@buoyant.io>; Tokio Contributors <team@tokio.rs>

### tracing-attributes 0.1.31

<!-- cargo-package: tracing-attributes 0.1.31 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/tracing
- **Homepage:** https://tokio.rs
- **Authors:** Tokio Contributors <team@tokio.rs>; Eliza Weisman <eliza@buoyant.io>; David Barsky <dbarsky@amazon.com>

### tracing-core 0.1.36

<!-- cargo-package: tracing-core 0.1.36 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/tokio-rs/tracing
- **Homepage:** https://tokio.rs
- **Authors:** Tokio Contributors <team@tokio.rs>

### tray-icon 0.25.1

<!-- cargo-package: tray-icon 0.25.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/tauri-apps/tray-icon
- **Homepage:** https://github.com/tauri-apps/tray-icon

### try-lock 0.2.5

<!-- cargo-package: try-lock 0.2.5 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/seanmonstar/try-lock
- **Homepage:** https://github.com/seanmonstar/try-lock
- **Authors:** Sean McArthur <sean@seanmonstar.com>

### type-map 0.5.1

<!-- cargo-package: type-map 0.5.1 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/kardeiz/type-map
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jacob Brown <kardeiz@gmail.com>

### typed-path 0.13.0

<!-- cargo-package: typed-path 0.13.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/chipsenkbeil/typed-path
- **Homepage:** https://github.com/chipsenkbeil/typed-path
- **Authors:** Chip Senkbeil <chip@senkbeil.org>

### typenum 1.20.1

<!-- cargo-package: typenum 1.20.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/paholg/typenum
- **Homepage:** Not specified in Cargo metadata

### uds_windows 1.2.1

<!-- cargo-package: uds_windows 1.2.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/haraldh/rust_uds_windows
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Azure IoT Edge Devs; Harald Hoyer <harald@redhat.com>

### unicode-general-category 1.1.0

<!-- cargo-package: unicode-general-category 1.1.0 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/yeslogic/unicode-general-category
- **Homepage:** https://github.com/yeslogic/unicode-general-category
- **Authors:** YesLogic Pty. Ltd. <info@yeslogic.com>

### unicode-ident 1.0.27

<!-- cargo-package: unicode-ident 1.0.27 -->

- **License (Cargo metadata):** `(MIT OR Apache-2.0) AND Unicode-3.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/dtolnay/unicode-ident
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### unicode-segmentation 1.13.3

<!-- cargo-package: unicode-segmentation 1.13.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/unicode-rs/unicode-segmentation
- **Homepage:** https://github.com/unicode-rs/unicode-segmentation
- **Authors:** kwantam <kwantam@gmail.com>; Manish Goregaokar <manishsmail@gmail.com>

### unicode-width 0.2.3

<!-- cargo-package: unicode-width 0.2.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/unicode-rs/unicode-width
- **Homepage:** https://github.com/unicode-rs/unicode-width
- **Authors:** kwantam <kwantam@gmail.com>; Manish Goregaokar <manishsmail@gmail.com>

### universal-hash 0.5.1

<!-- cargo-package: universal-hash 0.5.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/traits
- **Homepage:** Not specified in Cargo metadata
- **Authors:** RustCrypto Developers

### untrusted 0.9.0

<!-- cargo-package: untrusted 0.9.0 -->

- **License (Cargo metadata):** `ISC`
- **Repository:** https://github.com/briansmith/untrusted
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Brian Smith <brian@briansmith.org>

### ureq 3.5.0

<!-- cargo-package: ureq 3.5.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/algesten/ureq
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Martin Algesten <martin@algesten.se>; Jacob Hoffman-Andrews <ureq@hoffman-andrews.com>

### ureq-proto 0.6.5

<!-- cargo-package: ureq-proto 0.6.5 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/algesten/ureq-proto
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Martin Algesten <martin@algesten.se>

### url 2.5.8

<!-- cargo-package: url 2.5.8 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/servo/rust-url
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The rust-url developers

### utf8-zero 0.8.1

<!-- cargo-package: utf8-zero 0.8.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/algesten/utf8-zero
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Simon Sapin <simon.sapin@exyr.org>; Martin Algesten <martin@algesten.se>

### utf8_iter 1.1.0

<!-- cargo-package: utf8_iter 1.1.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/hsivonen/utf8_iter
- **Homepage:** https://docs.rs/utf8_iter/
- **Authors:** Henri Sivonen <hsivonen@hsivonen.fi>

### utf_types 1.0.0

<!-- cargo-package: utf_types 1.0.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/hsivonen/utf_types
- **Homepage:** https://docs.rs/utf_types/
- **Authors:** Henri Sivonen <hsivonen@hsivonen.fi>

### uuid 1.29.0

<!-- cargo-package: uuid 1.29.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/uuid-rs/uuid
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Ashley Mannix<ashleymannix@live.com.au>; Dylan DPC<dylan.dpc@gmail.com>; Hunar Roop Kahlon<hunar.roop@gmail.com>

### vello_common 0.0.9

<!-- cargo-package: vello_common 0.0.9 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/vello
- **Homepage:** Not specified in Cargo metadata

### vello_cpu 0.0.9

<!-- cargo-package: vello_cpu 0.0.9 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/linebender/vello
- **Homepage:** Not specified in Cargo metadata

### version_check 0.9.5

<!-- cargo-package: version_check 0.9.5 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/SergioBenitez/version_check
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Sergio Benitez <sb@sergio.bz>

### walkdir 2.5.0

<!-- cargo-package: walkdir 2.5.0 -->

- **License (Cargo metadata):** `Unlicense/MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/BurntSushi/walkdir
- **Homepage:** https://github.com/BurntSushi/walkdir
- **Authors:** Andrew Gallant <jamslam@gmail.com>

### want 0.3.2

<!-- cargo-package: want 0.3.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/seanmonstar/want
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Sean McArthur <sean@seanmonstar.com>

### wasapi 0.26.0

<!-- cargo-package: wasapi 0.26.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/HEnquist/wasapi-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** HEnquist <henrik.enquist@gmail.com>

### wasi 0.11.1+wasi-snapshot-preview1

<!-- cargo-package: wasi 0.11.1+wasi-snapshot-preview1 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/wasi
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The Cranelift Project Developers

### wasip2 1.0.4+wasi-0.2.12

<!-- cargo-package: wasip2 1.0.4+wasi-0.2.12 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/wasi-rs
- **Homepage:** Not specified in Cargo metadata

### wasm-bindgen 0.2.130

<!-- cargo-package: wasm-bindgen 0.2.130 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/wasm-bindgen/wasm-bindgen
- **Homepage:** https://wasm-bindgen.github.io/wasm-bindgen
- **Authors:** The wasm-bindgen Developers

### wasm-bindgen-futures 0.4.80

<!-- cargo-package: wasm-bindgen-futures 0.4.80 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/futures
- **Homepage:** https://wasm-bindgen.github.io/wasm-bindgen/
- **Authors:** The wasm-bindgen Developers

### wasm-bindgen-macro 0.2.130

<!-- cargo-package: wasm-bindgen-macro 0.2.130 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro
- **Homepage:** https://wasm-bindgen.github.io/wasm-bindgen/
- **Authors:** The wasm-bindgen Developers

### wasm-bindgen-macro-support 0.2.130

<!-- cargo-package: wasm-bindgen-macro-support 0.2.130 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/wasm-bindgen/wasm-bindgen/tree/main/crates/macro-support
- **Homepage:** https://wasm-bindgen.github.io/wasm-bindgen/
- **Authors:** The wasm-bindgen Developers

### wasm-bindgen-shared 0.2.130

<!-- cargo-package: wasm-bindgen-shared 0.2.130 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared
- **Homepage:** https://wasm-bindgen.github.io/wasm-bindgen/
- **Authors:** The wasm-bindgen Developers

### wayland-backend 0.3.17

<!-- cargo-package: wayland-backend 0.3.17 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-client 0.31.15

<!-- cargo-package: wayland-client 0.31.15 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-csd-frame 0.3.0

<!-- cargo-package: wayland-csd-frame 0.3.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/rust-windowing/wayland-csd-frame
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Kirill Chibisov <contact@kchibisov.com>

### wayland-cursor 0.31.14

<!-- cargo-package: wayland-cursor 0.31.14 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-protocols 0.32.13

<!-- cargo-package: wayland-protocols 0.32.13 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-protocols-experimental 20250721.0.1

<!-- cargo-package: wayland-protocols-experimental 20250721.0.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-protocols-misc 0.3.12

<!-- cargo-package: wayland-protocols-misc 0.3.12 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-protocols-plasma 0.3.12

<!-- cargo-package: wayland-protocols-plasma 0.3.12 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-protocols-wlr 0.3.12

<!-- cargo-package: wayland-protocols-wlr 0.3.12 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-scanner 0.31.11

<!-- cargo-package: wayland-scanner 0.31.11 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### wayland-sys 0.31.11

<!-- cargo-package: wayland-sys 0.31.11 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/smithay/wayland-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Elinor Berger <elinor@safaradeg.net>

### web-sys 0.3.107

<!-- cargo-package: web-sys 0.3.107 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/web-sys
- **Homepage:** https://wasm-bindgen.github.io/wasm-bindgen/web-sys/index.html
- **Authors:** The wasm-bindgen Developers

### web-time 1.1.0

<!-- cargo-package: web-time 1.1.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/daxpedda/web-time
- **Homepage:** Not specified in Cargo metadata

### webbrowser 1.2.6

<!-- cargo-package: webbrowser 1.2.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/amodm/webbrowser-rs
- **Homepage:** https://github.com/amodm/webbrowser-rs
- **Authors:** Amod Malviya @amodm

### webpki-roots 1.0.11

<!-- cargo-package: webpki-roots 1.0.11 -->

- **License (Cargo metadata):** `CDLA-Permissive-2.0`
- **Repository:** https://github.com/rustls/webpki-roots
- **Homepage:** https://github.com/rustls/webpki-roots

### weezl 0.1.12

<!-- cargo-package: weezl 0.1.12 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/image-rs/weezl
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The image-rs Developers

### wgpu 29.0.4

<!-- cargo-package: wgpu 29.0.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/gfx-rs/wgpu
- **Homepage:** https://wgpu.rs/
- **Authors:** gfx-rs developers

### wgpu-core 29.0.4

<!-- cargo-package: wgpu-core 29.0.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/gfx-rs/wgpu
- **Homepage:** https://wgpu.rs/
- **Authors:** gfx-rs developers

### wgpu-core-deps-windows-linux-android 29.0.4

<!-- cargo-package: wgpu-core-deps-windows-linux-android 29.0.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/gfx-rs/wgpu
- **Homepage:** https://wgpu.rs/
- **Authors:** gfx-rs developers

### wgpu-hal 29.0.4

<!-- cargo-package: wgpu-hal 29.0.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/gfx-rs/wgpu
- **Homepage:** https://wgpu.rs/
- **Authors:** gfx-rs developers

### wgpu-naga-bridge 29.0.4

<!-- cargo-package: wgpu-naga-bridge 29.0.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/gfx-rs/wgpu
- **Homepage:** https://wgpu.rs/
- **Authors:** gfx-rs developers

### wgpu-types 29.0.4

<!-- cargo-package: wgpu-types 29.0.4 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/gfx-rs/wgpu
- **Homepage:** https://wgpu.rs/
- **Authors:** gfx-rs developers

### winapi 0.3.9

<!-- cargo-package: winapi 0.3.9 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/retep998/winapi-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Peter Atashian <retep998@gmail.com>

### winapi-i686-pc-windows-gnu 0.4.0

<!-- cargo-package: winapi-i686-pc-windows-gnu 0.4.0 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/retep998/winapi-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Peter Atashian <retep998@gmail.com>

### winapi-util 0.1.11

<!-- cargo-package: winapi-util 0.1.11 -->

- **License (Cargo metadata):** `Unlicense OR MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/BurntSushi/winapi-util
- **Homepage:** https://github.com/BurntSushi/winapi-util
- **Authors:** Andrew Gallant <jamslam@gmail.com>

### winapi-x86_64-pc-windows-gnu 0.4.0

<!-- cargo-package: winapi-x86_64-pc-windows-gnu 0.4.0 -->

- **License (Cargo metadata):** `MIT/Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/retep998/winapi-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Peter Atashian <retep998@gmail.com>

### windows 0.62.2

<!-- cargo-package: windows 0.62.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-collections 0.3.2

<!-- cargo-package: windows-collections 0.3.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-core 0.62.2

<!-- cargo-package: windows-core 0.62.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-future 0.3.2

<!-- cargo-package: windows-future 0.3.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-implement 0.60.2

<!-- cargo-package: windows-implement 0.60.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-interface 0.59.3

<!-- cargo-package: windows-interface 0.59.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-link 0.2.1

<!-- cargo-package: windows-link 0.2.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-numerics 0.3.1

<!-- cargo-package: windows-numerics 0.3.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-result 0.4.1

<!-- cargo-package: windows-result 0.4.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-strings 0.5.1

<!-- cargo-package: windows-strings 0.5.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-sys 0.52.0

<!-- cargo-package: windows-sys 0.52.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows-sys 0.59.0

<!-- cargo-package: windows-sys 0.59.0 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows-sys 0.61.2

<!-- cargo-package: windows-sys 0.61.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows-targets 0.52.6

<!-- cargo-package: windows-targets 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows-threading 0.2.1

<!-- cargo-package: windows-threading 0.2.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata

### windows_aarch64_gnullvm 0.52.6

<!-- cargo-package: windows_aarch64_gnullvm 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows_aarch64_msvc 0.52.6

<!-- cargo-package: windows_aarch64_msvc 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows_i686_gnu 0.52.6

<!-- cargo-package: windows_i686_gnu 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows_i686_gnullvm 0.52.6

<!-- cargo-package: windows_i686_gnullvm 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows_i686_msvc 0.52.6

<!-- cargo-package: windows_i686_msvc 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows_x86_64_gnu 0.52.6

<!-- cargo-package: windows_x86_64_gnu 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows_x86_64_gnullvm 0.52.6

<!-- cargo-package: windows_x86_64_gnullvm 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### windows_x86_64_msvc 0.52.6

<!-- cargo-package: windows_x86_64_msvc 0.52.6 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/microsoft/windows-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Microsoft

### winit 0.30.13

<!-- cargo-package: winit 0.30.13 -->

- **License (Cargo metadata):** `Apache-2.0`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/rust-windowing/winit
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The winit contributors; Pierre Krieger <pierre.krieger1708@gmail.com>

### winnow 0.7.15

<!-- cargo-package: winnow 0.7.15 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/winnow-rs/winnow
- **Homepage:** Not specified in Cargo metadata

### winnow 1.0.4

<!-- cargo-package: winnow 1.0.4 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/winnow-rs/winnow
- **Homepage:** Not specified in Cargo metadata

### winresource 0.1.34

<!-- cargo-package: winresource 0.1.34 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/BenjaminRi/winresource
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Max Resch <resch.max@gmail.com>

### winx 0.36.4

<!-- cargo-package: winx 0.36.4 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception`; see [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/sunfishcode/winx
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Jakub Konka <kubkon@jakubkonka.com>; Dan Gohman <dev@sunfishcode.online>

### wit-bindgen 0.57.1

<!-- cargo-package: wit-bindgen 0.57.1 -->

- **License (Cargo metadata):** `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/bytecodealliance/wit-bindgen
- **Homepage:** https://github.com/bytecodealliance/wit-bindgen
- **Authors:** Alex Crichton <alex@alexcrichton.com>

### writeable 0.6.4

<!-- cargo-package: writeable 0.6.4 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The ICU4X Project Developers

### wyz 0.5.1

<!-- cargo-package: wyz 0.5.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/myrrlyn/wyz
- **Homepage:** https://myrrlyn.net/crates/wyz
- **Authors:** myrrlyn <self@myrrlyn.dev>

### x11-dl 2.21.0

<!-- cargo-package: x11-dl 2.21.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/AltF02/x11-rs.git
- **Homepage:** Not specified in Cargo metadata
- **Authors:** daggerbot <daggerbot@gmail.com>; Erle Pereira <erle@erlepereira.com>; AltF02 <contact@altf2.dev>

### x11rb 0.13.2

<!-- cargo-package: x11rb 0.13.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/psychon/x11rb
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Uli Schlachter <psychon@znc.in>; Eduardo Sánchez Muñoz <eduardosm-dev@e64.io>; notgull <jtnunley01@gmail.com>

### x11rb-protocol 0.13.2

<!-- cargo-package: x11rb-protocol 0.13.2 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/psychon/x11rb
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Uli Schlachter <psychon@znc.in>; Eduardo Sánchez Muñoz <eduardosm-dev@e64.io>; notgull <jtnunley01@gmail.com>

### xcursor 0.3.11

<!-- cargo-package: xcursor 0.3.11 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/esposm03/xcursor-rs
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Samuele Esposito

### xkbcommon-dl 0.4.2

<!-- cargo-package: xkbcommon-dl 0.4.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/rust-windowing/xkbcommon-dl
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Francesca Frangipane <francesca@comfysoft.net>

### xkeysym 0.2.1

<!-- cargo-package: xkeysym 0.2.1 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR Zlib`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/notgull/xkeysym
- **Homepage:** Not specified in Cargo metadata
- **Authors:** John Nunley <jtnunley01@gmail.com>

### xml-rs 0.8.29

<!-- cargo-package: xml-rs 0.8.29 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/kornelski/xml-rs
- **Homepage:** https://lib.rs/crates/xml
- **Authors:** Vladimir Matveev <vmatveev@citrine.cc>

### yoke 0.8.4

<!-- cargo-package: yoke 0.8.4 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Manish Goregaokar <manishsmail@gmail.com>

### yoke-derive 0.8.4

<!-- cargo-package: yoke-derive 0.8.4 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Manish Goregaokar <manishsmail@gmail.com>

### zbus 5.19.0

<!-- cargo-package: zbus 5.19.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/z-galaxy/zbus/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Zeeshan Ali Khan <zeeshanak@gnome.org>

### zbus_macros 5.19.0

<!-- cargo-package: zbus_macros 5.19.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/z-galaxy/zbus/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Marc-André Lureau <marcandre.lureau@redhat.com>; Zeeshan Ali Khan <zeeshanak@gnome.org>

### zbus_names 4.3.5

<!-- cargo-package: zbus_names 4.3.5 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/z-galaxy/zbus/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Zeeshan Ali Khan <zeeshanak@gnome.org>

### zcheapstr 1.2.0

<!-- cargo-package: zcheapstr 1.2.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/z-galaxy/zcheapstr/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Zeeshan Ali Khan <zeeshanak@gnome.org>

### zerocopy 0.8.63

<!-- cargo-package: zerocopy 0.8.63 -->

- **License (Cargo metadata):** `BSD-2-Clause OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/google/zerocopy
- **Homepage:** Not specified in Cargo metadata

### zerocopy-derive 0.8.63

<!-- cargo-package: zerocopy-derive 0.8.63 -->

- **License (Cargo metadata):** `BSD-2-Clause OR Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/google/zerocopy
- **Homepage:** Not specified in Cargo metadata

### zerofrom 0.1.8

<!-- cargo-package: zerofrom 0.1.8 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The ICU4X Project Developers

### zerofrom-derive 0.1.8

<!-- cargo-package: zerofrom-derive 0.1.8 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Manish Goregaokar <manishsmail@gmail.com>

### zeroize 1.9.1

<!-- cargo-package: zeroize 1.9.1 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/utils
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The RustCrypto Project Developers

### zeroize_derive 1.5.0

<!-- cargo-package: zeroize_derive 1.5.0 -->

- **License (Cargo metadata):** `Apache-2.0 OR MIT`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/RustCrypto/utils
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The RustCrypto Project Developers

### zerotrie 0.2.5

<!-- cargo-package: zerotrie 0.2.5 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** https://icu4x.unicode.org
- **Authors:** The ICU4X Project Developers

### zerovec 0.11.9

<!-- cargo-package: zerovec 0.11.9 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** The ICU4X Project Developers

### zerovec-derive 0.11.7

<!-- cargo-package: zerovec-derive 0.11.7 -->

- **License (Cargo metadata):** `Unicode-3.0`
- **Repository:** https://github.com/unicode-org/icu4x
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Manish Goregaokar <manishsmail@gmail.com>

### zip 9.4.2

<!-- cargo-package: zip 9.4.2 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/zip-rs/zip2
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Mathijs van de Nes <git@mathijs.vd-nes.nl>; Marli Frost <marli@frost.red>; Ryan Levick <ryan.levick@gmail.com>; Chris Hennick <hennickc@amazon.com>

### zlib-rs 0.6.8

<!-- cargo-package: zlib-rs 0.6.8 -->

- **License (Cargo metadata):** `Zlib`
- **Repository:** https://github.com/trifectatechfoundation/zlib-rs
- **Homepage:** https://github.com/trifectatechfoundation/zlib-rs

### zmij 1.0.23

<!-- cargo-package: zmij 1.0.23 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/dtolnay/zmij
- **Homepage:** Not specified in Cargo metadata
- **Authors:** David Tolnay <dtolnay@gmail.com>

### zune-core 0.5.3

<!-- cargo-package: zune-core 0.5.3 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR Zlib`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/etemesi254/zune-image
- **Homepage:** Not specified in Cargo metadata

### zune-jpeg 0.5.15

<!-- cargo-package: zune-jpeg 0.5.15 -->

- **License (Cargo metadata):** `MIT OR Apache-2.0 OR Zlib`; see [MIT license terms](#appendix-a-mit-license) and [Apache-2.0 license terms](#appendix-b-apache-20-license)
- **Repository:** https://github.com/etemesi254/zune-image/tree/dev/crates/zune-jpeg
- **Homepage:** Not specified in Cargo metadata
- **Authors:** caleb <etemesicaleb@gmail.com>

### zvariant 5.15.1

<!-- cargo-package: zvariant 5.15.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/z-galaxy/zbus/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Zeeshan Ali Khan <zeeshanak@gnome.org>

### zvariant_derive 5.15.1

<!-- cargo-package: zvariant_derive 5.15.1 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/z-galaxy/zbus/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Zeeshan Ali Khan <zeeshanak@gnome.org>

### zvariant_utils 4.2.0

<!-- cargo-package: zvariant_utils 4.2.0 -->

- **License (Cargo metadata):** `MIT`; see [MIT license terms](#appendix-a-mit-license)
- **Repository:** https://github.com/z-galaxy/zbus/
- **Homepage:** Not specified in Cargo metadata
- **Authors:** Zeeshan Ali Khan <zeeshanak@gnome.org>; turbocooler <turbocooler@cocaine.ninja>


## Appendix A: MIT License

The common MIT terms are reproduced here once. Package-specific vendored copies in Appendix C preserve their original copyright lines and formatting.

```text
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Appendix B: Apache-2.0 License

This is the exact `vendor/egui_software_backend/LICENSE-APACHE` text and the standard Apache License, Version 2.0 terms referenced by packages whose metadata includes Apache-2.0.

```text
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS
```

## Appendix C: Vendored License and NOTICE Texts

The following texts are copied verbatim from the repository. Each file appears once; packages and bundled components above refer to these copies where applicable.

### `assets/third-party/file-transfer/cap-std-LICENSE-MIT`

**Applies to:** cap-std and cap-fs-ext (MIT option)

```text
Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### `assets/third-party/file-transfer/fuser-LICENSE-MIT`

**Applies to:** fuser (MIT option)

```text
The MIT License (MIT)
=====================

Copyright (c) 2020-present Christopher Berner

Copyright © `2013-2019` `Andreas Neuhaus` `https://zargony.com/`

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the “Software”), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

### `assets/third-party/file-transfer/nix-LICENSE-MIT`

**Applies to:** nix (MIT option)

```text
The MIT License (MIT)

Copyright (c) 2015 Carl Lerche + nix-rust Authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### `assets/third-party/file-transfer/page-size-LICENSE-MIT`

**Applies to:** page_size (MIT option)

```text
Copyright (c) 2014 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### `assets/third-party/file-transfer/ref-cast-LICENSE-MIT`

**Applies to:** ref-cast and ref-cast-impl (MIT option)

```text
Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### `assets/third-party/file-transfer/rfd-LICENSE`

**Applies to:** rfd

```text
MIT License

Copyright (c) 2022 Bartłomiej Maryńczak

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### `assets/third-party/file-transfer/rfd-window-identifier-LICENSE`

**Applies to:** rfd XDG portal window identifier support

```text
MIT License

Copyright (c) 2020 Bilal Elmoussaoui

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### `assets/third-party/vb-cable/NOTICE.txt`

**Applies to:** VB-CABLE Virtual Audio Device

```text
VB-CABLE Virtual Audio Device — original VB-Audio installer archives

VB-CABLE is developed and distributed by VB-Audio Software.
Official website: https://www.vb-cable.com/
Downloads and installation help: https://vb-audio.com/Cable/
Support VB-Audio and donate: https://vb-audio.com/Donate/index.htm

VB-CABLE is donationware, not part of Desk Switch's MIT-licensed code.
The ordinary VB-CABLE archives are bundled AS IS, including the original
installers and readme files, under VB-Audio's redistribution terms:
https://vb-audio.com/Services/licensing.htm
The separately licensed VB-CABLE A+B and C+D packages are not included.

Desk Switch extracts these files only at your request. It does not install
a driver automatically. Follow the original readme for your OS and keep
your normal headphones as the system output. The macOS archive is a backup
installer; Desk Switch itself currently supports Windows and Ubuntu.

Downloaded from VB-Audio on 2026-11-07:
Windows: VBCABLE_Driver_Pack46.zip
https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip
SHA-256: bc44e8974191a5a2427d6c979e2dab08dbb09925f3f435dfa2f0ba3e45b6eb90
macOS: VBCable_MACDriver_Pack108.zip
https://download.vb-audio.com/Download_MAC/VBCable_MACDriver_Pack108.zip
SHA-256: e46b41c6876995403cb1da37d7c0d566f59edd83aa9fcbcdc3a205eb0b6e05c7
```

### `vendor/egui_software_backend/LICENSE-MIT`

**Applies to:** egui_software_backend (MIT option)

```text
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### `vendor/m1ddc/LICENSE`

**Applies to:** m1ddc

```text
MIT License

Copyright (c) 2021 waydabber

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### `vendor/rust-auto-updator/LICENSE`

**Applies to:** rust-auto-updator

```text
MIT License

Copyright (c) 2026 siarheisinelnikau

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
