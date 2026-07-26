# Checks the hole-punch without relying on a screenshot.
#
# Walks the top-level windows of the running demo, finds the airspace host child
# window, and reads its window region back with GetRegionData. A host with one
# hole comes back as several rectangles adding up to client rect minus the hole,
# which is what Windows will actually clip to.

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class Rgn {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", SetLastError=true)]
    public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string win);
    [DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr hWnd, IntPtr hRgn);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
    [DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int l, int t, int r, int b);
    [DllImport("gdi32.dll")] public static extern uint GetRegionData(IntPtr hRgn, uint count, IntPtr data);
    [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr o);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int left, top, right, bottom; }

    public static List<IntPtr> TopLevel(uint pid) {
        var found = new List<IntPtr>();
        EnumWindows((h, p) => {
            uint wpid; GetWindowThreadProcessId(h, out wpid);
            if (wpid == pid) found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static string Dump(uint pid) {
        var sb = new StringBuilder();
        bool any = false;
        foreach (IntPtr top in TopLevel(pid)) {
            IntPtr child = FindWindowEx(top, IntPtr.Zero, "tauri_airspace_host", null);
            if (child == IntPtr.Zero) continue;
            any = true;
            RECT cr; GetClientRect(child, out cr);
            sb.AppendLine("parent hwnd    : 0x" + top.ToInt64().ToString("x"));
            sb.AppendLine("host hwnd      : 0x" + child.ToInt64().ToString("x"));
            sb.AppendLine("host client    : " + cr.right + "x" + cr.bottom);

            IntPtr rgn = CreateRectRgn(0,0,1,1);
            if (GetWindowRgn(child, rgn) == 0) {
                sb.AppendLine("region         : NONE (host is a plain rectangle - no holes)");
                DeleteObject(rgn);
                continue;
            }
            uint size = GetRegionData(rgn, 0, IntPtr.Zero);
            IntPtr buf = Marshal.AllocHGlobal((int)size);
            GetRegionData(rgn, size, buf);
            int count = Marshal.ReadInt32(buf, 8);   // RGNDATAHEADER.nCount
            sb.AppendLine("region rects   : " + count);
            int off = 32;                            // sizeof(RGNDATAHEADER)
            for (int i = 0; i < count; i++) {
                int l = Marshal.ReadInt32(buf, off + i*16 + 0);
                int t = Marshal.ReadInt32(buf, off + i*16 + 4);
                int r = Marshal.ReadInt32(buf, off + i*16 + 8);
                int b = Marshal.ReadInt32(buf, off + i*16 + 12);
                sb.AppendLine(String.Format("  rect {0,-2}      : ({1},{2})-({3},{4})  {5}x{6}", i, l, t, r, b, r-l, b-t));
            }
            Marshal.FreeHGlobal(buf);
            DeleteObject(rgn);
        }
        if (!any) sb.AppendLine("NO HOST CHILD WINDOW FOUND (is the native content started?)");
        return sb.ToString();
    }
}
'@

$procs = Get-Process airspace-demo -ErrorAction SilentlyContinue
if (-not $procs) { Write-Output "airspace-demo is not running"; exit 1 }
foreach ($proc in $procs) {
    Write-Output ("--- pid {0}" -f $proc.Id)
    Write-Output ([Rgn]::Dump([uint32]$proc.Id))
}
