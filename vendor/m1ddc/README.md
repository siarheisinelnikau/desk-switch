# Apple Silicon DDC transport

The MIT-licensed transport files are vendored from [waydabber/m1ddc](https://github.com/waydabber/m1ddc), commit `7339ceebb03f691b1eaeb8471dca301d215d19c8`. See `LICENSE`.

Desk Switch changes `DDC_ITERATIONS` to **1** and raises the enumeration bound to 128 displays. The application uses its own narrowly scoped `DisplayDDC.m` entry point: select an explicit display identity, send one VCP `0x60` selection, and never verify by repeating the write. No automatic alternate input codes or chip-address probes are used. An explicit read is a separate operation. The Apple Silicon transport relies on private macOS APIs and may be unavailable after an OS update; unsupported transports produce a visible error.
