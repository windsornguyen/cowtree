$ErrorActionPreference = "Stop"
# Use only this job's new virtual disk; never format a runner's existing volume.
$image = Join-Path $env:RUNNER_TEMP "cowtree-native.vhdx"
$script = Join-Path $env:RUNNER_TEMP "cowtree-diskpart.txt"
if (Test-Path R:\) { throw "Drive R is already in use" }
if (Test-Path $image) { throw "Test disk already exists" }
@(
    ('create vdisk file="{0}" maximum=1024 type=expandable' -f $image)
    ('select vdisk file="{0}"' -f $image)
    "attach vdisk"
    "create partition primary"
    "format fs=refs quick label=CowtreeTest"
    "assign letter=R"
) | Set-Content -Path $script -Encoding ascii
try {
    diskpart /s $script
    if ($LASTEXITCODE -ne 0) { throw "Disk creation failed" }
    if ((Get-Volume -DriveLetter R).FileSystem -ne "ReFS") { throw "ReFS was not mounted" }
    $env:COWTREE_NTFS_TEST = Join-Path $env:RUNNER_TEMP "cowtree-ntfs"
    uv run --no-sync pytest -q windows --basetemp R:\cowtree-tests
    if ($LASTEXITCODE -ne 0) { throw "Native Windows checks failed" }
} finally {
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
