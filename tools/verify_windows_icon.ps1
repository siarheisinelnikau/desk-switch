[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [string]$ExpectedIcon = (Join-Path $PSScriptRoot '../assets/desk-switch.ico'),
    [string]$ExpectedVersion,
    [string]$PngDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') {
    throw 'Native executable icon verification requires Windows.'
}

Add-Type -AssemblyName System.Drawing
if (-not ('DeskSwitchReleaseIconNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class DeskSwitchReleaseIconNative {
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern uint ExtractIconExW(string file, int index,
        out IntPtr large, out IntPtr small, uint count);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool DestroyIcon(IntPtr icon);
}
'@
}

function Read-NativeIcons([string]$Path) {
    $large = [IntPtr]::Zero
    $small = [IntPtr]::Zero
    $bitmaps = @{}
    $groups = [DeskSwitchReleaseIconNative]::ExtractIconExW($Path, -1, [ref]$large, [ref]$small, 0)
    if ($groups -lt 1) {
        throw "No native icon resources in $Path"
    }
    try {
        $loaded = [DeskSwitchReleaseIconNative]::ExtractIconExW($Path, 0, [ref]$large, [ref]$small, 1)
        if ($loaded -lt 1 -or $large -eq [IntPtr]::Zero -or $small -eq [IntPtr]::Zero) {
            throw "Cannot load both native large and small icons from $Path"
        }
        foreach ($slot in @(@('large', $large), @('small', $small))) {
            $icon = [Drawing.Icon]::FromHandle($slot[1])
            try {
                $bitmaps[$slot[0]] = $icon.ToBitmap()
            }
            finally {
                $icon.Dispose()
            }
        }
        return @{ Groups = $groups; Bitmaps = $bitmaps }
    }
    catch {
        foreach ($bitmap in $bitmaps.Values) { $bitmap.Dispose() }
        throw
    }
    finally {
        if ($large -ne [IntPtr]::Zero) { [void][DeskSwitchReleaseIconNative]::DestroyIcon($large) }
        if ($small -ne [IntPtr]::Zero) { [void][DeskSwitchReleaseIconNative]::DestroyIcon($small) }
    }
}

$exe = (Get-Item -LiteralPath $Executable).FullName
$reference = (Get-Item -LiteralPath $ExpectedIcon).FullName
if ([string]::IsNullOrWhiteSpace($ExpectedVersion)) {
    $cargo = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../Cargo.toml'))
    $package = [regex]::Match($cargo, '(?ms)^\[package\]\s*(.*?)(?=^\[|\z)').Groups[1].Value
    $versionMatch = [regex]::Match($package, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$')
    if (-not $versionMatch.Success) { throw 'Cannot read the stable package version from Cargo.toml.' }
    $ExpectedVersion = $versionMatch.Groups[1].Value
}
if ($ExpectedVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    throw 'ExpectedVersion must be a stable semantic version.'
}
$version = [Diagnostics.FileVersionInfo]::GetVersionInfo($exe)
$versionPattern = '^' + [regex]::Escape($ExpectedVersion) + '(?:\.0)?$'
$expectedName = if ([version]$ExpectedVersion -lt [version]'0.8.1') {
    "desk-switch-v$ExpectedVersion-windows-x86-64.exe"
} else {
    "desk-switch-v$ExpectedVersion.exe"
}
if ($version.ProductName -ne 'Desk Switch' -or $version.FileDescription -ne 'Desk Switch' -or
    $version.ProductVersion -notmatch $versionPattern -or $version.FileVersion -notmatch $versionPattern -or
    $version.OriginalFilename -ne $expectedName) {
    throw "Executable branding/version metadata does not match Desk Switch $ExpectedVersion ($expectedName)."
}
if ($PngDirectory) {
    $PngDirectory = [IO.Path]::GetFullPath($PngDirectory)
    [void][IO.Directory]::CreateDirectory($PngDirectory)
}

$actual = $null
$expected = $null
$proof = @()
try {
    $actual = Read-NativeIcons $exe
    $expected = Read-NativeIcons $reference
    foreach ($slot in @('large', 'small')) {
        $bitmap = $actual.Bitmaps[$slot]
        $original = $expected.Bitmaps[$slot]
        $size = if ($slot -eq 'large') { 32 } else { 16 }
        if ($bitmap.Width -ne $size -or $bitmap.Height -ne $size -or
            $original.Width -ne $size -or $original.Height -ne $size) {
            throw "Native $slot icon did not load at $size x $size pixels."
        }
        $visible = 0
        $different = 0
        for ($y = 0; $y -lt $size; $y++) {
            for ($x = 0; $x -lt $size; $x++) {
                $pixel = $bitmap.GetPixel($x, $y)
                $wanted = $original.GetPixel($x, $y)
                if ($pixel.A -gt 0) { $visible++ }
                # RGB values under complete transparency are not visible artwork.
                if ($pixel.A -ne $wanted.A -or ($pixel.A -gt 0 -and
                    ($pixel.R -ne $wanted.R -or $pixel.G -ne $wanted.G -or $pixel.B -ne $wanted.B))) {
                    $different++
                }
            }
        }
        if ($visible -lt ($size * $size / 10)) { throw "Native $slot icon is blank or nearly transparent." }
        if ($different -gt 0) { throw "Native $slot icon differs from the original artwork at $different pixels." }
        if ($PngDirectory) {
            $bitmap.Save((Join-Path $PngDirectory "native-$slot.png"), [Drawing.Imaging.ImageFormat]::Png)
        }
        $proof += [pscustomobject]@{ Slot = $slot; Width = $size; Height = $size; VisiblePixels = $visible; DifferentPixels = $different }
    }
    [pscustomobject]@{
        Executable = $exe
        ProductVersion = $version.ProductVersion
        FileVersion = $version.FileVersion
        OriginalFilename = $version.OriginalFilename
        ExecutableIconGroups = $actual.Groups
        ReferenceIconGroups = $expected.Groups
        Icons = $proof
    } | ConvertTo-Json -Depth 4
}
finally {
    if ($actual) { foreach ($bitmap in $actual.Bitmaps.Values) { $bitmap.Dispose() } }
    if ($expected) { foreach ($bitmap in $expected.Bitmaps.Values) { $bitmap.Dispose() } }
}
