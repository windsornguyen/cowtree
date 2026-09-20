param([Parameter(Mandatory=$true)][string]$Directory)
$ErrorActionPreference = 'Stop'

# Native primitive qualification only. This does not port the managed workspace API.
# https://learn.microsoft.com/en-us/windows/win32/fileio/block-cloning
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class CowtreeRefsProbe {
    [StructLayout(LayoutKind.Sequential)]
    struct Extents { public IntPtr Source; public long From; public long To; public long Length; }
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool DeviceIoControl(SafeFileHandle file, uint code, ref Extents extents,
        int size, IntPtr output, int outputSize, out int returned, IntPtr overlapped);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool GetVolumeInformation(string root, StringBuilder label, int labelSize,
        out uint serial, out uint component, out uint flags, StringBuilder fs, int fsSize);
    public static string Run(string directory) {
        var root = Path.GetPathRoot(Path.GetFullPath(directory));
        uint serial, component, flags;
        var fs = new StringBuilder(128);
        if (!GetVolumeInformation(root, null, 0, out serial, out component, out flags, fs, fs.Capacity))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        if ((flags & 0x08000000) == 0) throw new IOException("volume lacks block refcounting");
        if (Directory.Exists(directory)) throw new IOException("probe directory must be new");
        Directory.CreateDirectory(directory);
        var source = Path.Combine(directory, "source.bin");
        var target = Path.Combine(directory, "clone.bin");
        var bytes = new byte[8 * 1024 * 1024];
        new Random(429).NextBytes(bytes);
        using (var output = new FileStream(source, FileMode.CreateNew, FileAccess.Write)) {
            output.Write(bytes, 0, bytes.Length); output.Flush(true);
        }
        try {
            using (var input = new FileStream(source, FileMode.Open, FileAccess.Read, FileShare.Read))
            using (var output = new FileStream(target, FileMode.CreateNew, FileAccess.ReadWrite)) {
                output.SetLength(bytes.Length);
                var extents = new Extents {Source=input.SafeFileHandle.DangerousGetHandle(), Length=bytes.Length};
                int returned;
                // CTL_CODE(FILE_DEVICE_FILE_SYSTEM, 209, METHOD_BUFFERED, FILE_WRITE_DATA).
                if (!DeviceIoControl(output.SafeFileHandle, 0x00098344, ref extents,
                    Marshal.SizeOf(typeof(Extents)), IntPtr.Zero, 0, out returned, IntPtr.Zero))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                output.Flush(true);
            }
            using (var hash = SHA256.Create()) {
                var expected = Convert.ToBase64String(hash.ComputeHash(bytes));
                if (Convert.ToBase64String(hash.ComputeHash(File.ReadAllBytes(target))) != expected)
                    throw new IOException("clone bytes differ");
                using (var writer = new FileStream(source, FileMode.Open, FileAccess.Write)) {
                    writer.WriteByte((byte)(bytes[0] ^ 255)); writer.Flush(true);
                }
                if (Convert.ToBase64String(hash.ComputeHash(File.ReadAllBytes(target))) != expected)
                    throw new IOException("source mutation reached clone");
                using (var writer = new FileStream(target, FileMode.Open, FileAccess.Write)) {
                    writer.Position=4096; writer.WriteByte((byte)(bytes[4096] ^ 255)); writer.Flush(true);
                }
                var observed = File.ReadAllBytes(source);
                bytes[0] ^= 255;
                if (Convert.ToBase64String(hash.ComputeHash(observed)) != Convert.ToBase64String(hash.ComputeHash(bytes)))
                    throw new IOException("clone mutation reached source");
            }
            return fs.ToString();
        } finally {
            File.Delete(target); File.Delete(source); Directory.Delete(directory);
        }
    }
}
'@
$filesystem = [CowtreeRefsProbe]::Run($Directory)
@{ filesystem=$filesystem; bytes=8388608; clone='FSCTL_DUPLICATE_EXTENTS_TO_FILE'; bidirectionalIsolation=$true; temporaryFilesRemoved=$true } | ConvertTo-Json -Compress
