param([switch]$DevDrive)

$ErrorActionPreference = "Stop"
$temporary = $env:TEMP
$temporaryAlternate = $env:TMP
$originalUnsupported = $env:COWTREE_UNSUPPORTED_ROOT
# Use only this job's new virtual disk; never format a runner's existing volume.
$image = Join-Path $env:RUNNER_TEMP "cowtree-native.vhdx"
$script = Join-Path $env:RUNNER_TEMP "cowtree-diskpart.txt"
if (Test-Path R:\) { throw "Drive R is already in use" }
if (Test-Path $image) { throw "Test disk already exists" }
# Windows 11 Dev Drive requires at least 50 GiB; Server ReFS needs only 8 GiB.
$maximum = if ($DevDrive) { 65536 } else { 8192 }
$commands = @(
    ('create vdisk file="{0}" maximum={1} type=expandable' -f $image, $maximum)
    ('select vdisk file="{0}"' -f $image)
    "attach vdisk"
    "create partition primary"
)
if (-not $DevDrive) { $commands += "format fs=refs quick label=CowtreeTest" }
$commands += "assign letter=R"
$commands | Set-Content -Path $script -Encoding ascii
try {
    diskpart /s $script
    if ($LASTEXITCODE -ne 0) { throw "Disk creation failed" }
    if ($DevDrive) {
        Format-Volume -DriveLetter R -DevDrive -Force -Confirm:$false | Out-Null
    }
    if ((Get-Volume -DriveLetter R).FileSystem -ne "ReFS") { throw "ReFS was not mounted" }
    $env:COWTREE_UNSUPPORTED_ROOT = Join-Path $env:RUNNER_TEMP "cowtree-ntfs"
    $env:COWTREE_EXPECT_SUPPORTED = "1"
    $env:TEMP = "R:\rust-tests"
    $env:TMP = $env:TEMP
    New-Item -ItemType Directory -Path $env:TEMP | Out-Null
    cargo test --locked -p libcowtree
    if ($LASTEXITCODE -ne 0) { throw "Rust engine tests failed" }
    cargo build --locked --release -p cowtree-cli
    if ($LASTEXITCODE -ne 0) { throw "Native CLI build failed" }
    & target\release\cowtree.exe doctor R:\
    if ($LASTEXITCODE -ne 0) { throw "Native CLI probe failed" }
    cargo test --locked -p cowtree-cli --test standalone --test submodules
    if ($LASTEXITCODE -ne 0) { throw "Native executable checks failed" }
} finally {
    $env:TEMP = $temporary
    $env:TMP = $temporaryAlternate
    $env:COWTREE_UNSUPPORTED_ROOT = $originalUnsupported
    if (Test-Path $image) {
        @(
            ('select vdisk file="{0}"' -f $image)
            "detach vdisk"
        ) | Set-Content -Path $script -Encoding ascii
        diskpart /s $script
        if ($LASTEXITCODE -ne 0) { throw "Could not detach test disk: $image" }
        Remove-Item $image
    }
    Remove-Item $script
}
