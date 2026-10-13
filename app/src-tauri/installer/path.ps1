# Adds a folder to, or removes it from, the current user's PATH (the installer's "Add Subwoofa to PATH" step and the
# uninstaller's undo). It works on the registry value directly, so a long PATH is no problem, entries written with
# %VARIABLES% stay as they were, and nothing else in the PATH is changed. Only the current user's PATH is touched.
param(
    [Parameter(Mandatory)][ValidateSet("add", "remove")][string]$Action,
    [Parameter(Mandatory)][string]$Dir
)
$ErrorActionPreference = "Stop"

$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
try {
    $before = [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $same = { param($a, $b) $a.TrimEnd("\").ToLowerInvariant() -eq $b.TrimEnd("\").ToLowerInvariant() }
    $entries = @($before -split ";" | Where-Object { $_ -ne "" })
    $present = @($entries | Where-Object { & $same $_ $Dir }).Count -gt 0

    if ($Action -eq "add" -and -not $present) { $entries += $Dir }
    elseif ($Action -eq "remove" -and $present) { $entries = @($entries | Where-Object { -not (& $same $_ $Dir) }) }
    else { Write-Host "The PATH needs no change."; exit 0 }

    $key.SetValue("Path", ($entries -join ";"), [Microsoft.Win32.RegistryValueKind]::ExpandString)
}
finally { $key.Close() }

# tell running programs (Explorer, new terminals) that the environment changed
Add-Type -Namespace Win32 -Name Broadcast -MemberDefinition @"
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint flags, uint timeout, out System.UIntPtr result);
"@
$r = [UIntPtr]::Zero
[void][Win32.Broadcast]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, "Environment", 2, 5000, [ref]$r)
Write-Host ("Subwoofa was " + $(if ($Action -eq "add") { "added to" } else { "removed from" }) + " the PATH.")
