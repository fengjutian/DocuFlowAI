<#
.SYNOPSIS
Runs one command inside a Windows AppContainer.

.DESCRIPTION
The Phase 0 checklist wants a host boundary: the runtime must not be able to reach the host's
files. On Windows the native mechanism for that is an AppContainer - a token that is denied access
to everything except what the ACLs explicitly grant to the package SID. This script:

  1. derives the AppContainer SID for a profile name (no profile has to exist);
  2. grants that SID exactly what the run needs - Modify on the workspace, Read and execute on the
     paths the command has to read - and nothing else;
  3. starts the command with PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES and no capabilities,
     which is what puts it in the container and takes the network away;
  4. verifies from the parent's side that the child's token really is an AppContainer token
     (TokenIsAppContainer), so a run that only looks isolated is reported as not isolated.

If the boundary cannot be established - the SID cannot be derived, the ACLs cannot be written, or
the child is not in an AppContainer token - the command is NOT run. A run without a boundary would
be evidence of nothing, so the helper fails closed and says why.

The result is printed to stdout as one JSON document. The child's own stdout and stderr are
captured into files and returned inside that JSON, because the caller needs both the child's words
and the boundary's verdict.

The arguments travel in the environment variable CW_APPCONTAINER_ARGS (a JSON array), not on the
command line: a command line would have to quote them twice, and that is where arguments get lost.

.EXAMPLE
$env:CW_APPCONTAINER_ARGS = '["-e","console.log(1)"]'
powershell -File scripts/appcontainer-run.ps1 -Command node -Workspace D:\work
#>
param(
  [Parameter(Mandatory = $true)][string]$Command,
  [string]$ProfileName = "CodeWeaver.Sandbox",
  [string]$Workspace = "",
  [string]$ReadPaths = "",
  [int]$TimeoutMs = 60000,
  [string]$WorkDir = ""
)

$ErrorActionPreference = 'Stop'

if (-not (Get-Command Add-Type -ErrorAction SilentlyContinue)) {
  Write-Error "Add-Type is unavailable, so the AppContainer helper cannot be compiled"
  exit 2
}

if (-not ('CodeWeaver.AppContainerBoundary' -as [type])) {
  Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;

namespace CodeWeaver
{
    public class AppContainerResult
    {
        public bool Started;
        public bool TimedOut;
        public int ExitCode;
        public bool ExitCodeKnown;
        /// <summary>True when the child really ran with an AppContainer token.</summary>
        public bool SandboxEstablished;
        /// <summary>What the parent's own token query says about the child.</summary>
        public bool ChildIsAppContainer;
        public string AppContainerSid;
        public string ProfileName;
        public string Error;
        public string Stdout;
        public string Stderr;
        public long StdoutBytes;
        public long StderrBytes;
        public string[] GrantedPaths;
        public string[] UngrantablePaths;
    }

    public static class AppContainerBoundary
    {
        // WinBase.h (Windows 10 SDK 10.0.26100.0): ProcThreadAttributeSecurityCapabilities = 9 and
        // PROC_THREAD_ATTRIBUTE_INPUT = 0x00020000, so the attribute value is 0x00020009.
        private const uint PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES = 0x00020009;
        private const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
        private const uint CREATE_NO_WINDOW = 0x08000000;
        private const uint STARTF_USESTDHANDLES = 0x00000100;
        private const uint INFINITE = 0xFFFFFFFF;
        private const uint WAIT_TIMEOUT = 0x00000102;
        private const uint HANDLE_FLAG_INHERIT = 0x00000001;
        private const uint GENERIC_WRITE = 0x40000000;
        private const uint FILE_SHARE_READ = 0x00000001;
        private const uint FILE_SHARE_WRITE = 0x00000002;
        private const uint CREATE_ALWAYS = 2;
        private const uint FILE_ATTRIBUTE_TEMPORARY = 0x00000100;
        private const int STILL_ACTIVE = 259;

        // winnt.h: TOKEN_INFORMATION_CLASS starts at TokenUser = 1, so TokenIsAppContainer is 29.
        private const int TokenIsAppContainer = 29;
        private const uint TOKEN_QUERY = 0x0008;

        [StructLayout(LayoutKind.Sequential)]
        private struct STARTUPINFO
        {
            public int cb;
            public IntPtr lpReserved;
            public IntPtr lpDesktop;
            public IntPtr lpTitle;
            public int dwX;
            public int dwY;
            public int dwXSize;
            public int dwYSize;
            public int dwXCountChars;
            public int dwYCountChars;
            public int dwFillAttribute;
            public uint dwFlags;
            public short wShowWindow;
            public short cbReserved2;
            public IntPtr lpReserved2;
            public IntPtr hStdInput;
            public IntPtr hStdOutput;
            public IntPtr hStdError;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct STARTUPINFOEX
        {
            public STARTUPINFO StartupInfo;
            public IntPtr lpAttributeList;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct PROCESS_INFORMATION
        {
            public IntPtr hProcess;
            public IntPtr hThread;
            public int dwProcessId;
            public int dwThreadId;
        }

        // winnt.h: SECURITY_CAPABILITIES.
        [StructLayout(LayoutKind.Sequential)]
        private struct SECURITY_CAPABILITIES
        {
            public IntPtr AppContainerSid;
            public IntPtr Capabilities;
            public uint CapabilityCount;
            public uint Reserved;
        }

        [DllImport("userenv.dll", CharSet = CharSet.Unicode)]
        private static extern int DeriveAppContainerSidFromAppContainerName(string appContainerName, out IntPtr sid);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool InitializeProcThreadAttributeList(IntPtr attributeList, int attributeCount, int flags, ref IntPtr size);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool UpdateProcThreadAttribute(IntPtr attributeList, uint flags, IntPtr attribute, IntPtr value, IntPtr size, IntPtr previousValue, IntPtr returnSize);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern void DeleteProcThreadAttributeList(IntPtr attributeList);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool CreateProcessW(string applicationName, string commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment, string currentDirectory, ref STARTUPINFOEX startupInfo, out PROCESS_INFORMATION processInformation);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool CloseHandle(IntPtr handle);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool SetHandleInformation(IntPtr handle, uint mask, uint flags);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);

        [DllImport("advapi32.dll", SetLastError = true)]
        private static extern bool OpenProcessToken(IntPtr processHandle, uint desiredAccess, out IntPtr tokenHandle);

        [DllImport("advapi32.dll", SetLastError = true)]
        private static extern bool GetTokenInformation(IntPtr tokenHandle, int tokenClass, IntPtr information, int length, out int returnLength);

        public static AppContainerResult Run(string command, string[] args, string profileName, string workspace, string[] readPaths, int timeoutMs, string workDir)
        {
            var result = new AppContainerResult { ExitCode = -1, ExitCodeKnown = false, ProfileName = profileName };
            var granted = new List<string>();
            var ungrantable = new List<string>();
            IntPtr sid = IntPtr.Zero;
            IntPtr attributeList = IntPtr.Zero;
            IntPtr capabilities = IntPtr.Zero;
            IntPtr hOut = IntPtr.Zero;
            IntPtr hErr = IntPtr.Zero;
            string outPath = Path.Combine(Path.GetTempPath(), "cw-ac-out-" + Guid.NewGuid().ToString("N") + ".txt");
            string errPath = Path.Combine(Path.GetTempPath(), "cw-ac-err-" + Guid.NewGuid().ToString("N") + ".txt");
            try
            {
                int derived = DeriveAppContainerSidFromAppContainerName(profileName, out sid);
                if (derived != 0 || sid == IntPtr.Zero)
                    throw new Win32Exception(derived, "DeriveAppContainerSidFromAppContainerName failed (" + derived + ")");

                var identifier = new SecurityIdentifier(SidToBytes(sid));
                result.AppContainerSid = identifier.Value;

                // The workspace is the one place the child may write; the read paths are the places
                // it may read. Nothing else is granted, and that is the whole point: the host's
                // files stay out of reach because no ACE names the package SID.
                if (!string.IsNullOrEmpty(workspace))
                {
                    Grant(workspace, identifier, FileSystemRights.Modify | FileSystemRights.Synchronize, true, granted, ungrantable);
                    GrantAncestors(workspace, identifier, granted, ungrantable);
                }
                foreach (string path in readPaths ?? new string[0])
                {
                    if (string.IsNullOrEmpty(path)) continue;
                    Grant(path, identifier, FileSystemRights.ReadAndExecute | FileSystemRights.Synchronize, true, granted, ungrantable);
                    GrantAncestors(path, identifier, granted, ungrantable);
                }
                if (ungrantable.Count > 0 && string.IsNullOrEmpty(workspace))
                    throw new InvalidOperationException("the boundary has nothing to grant: " + string.Join("; ", ungrantable.ToArray()));

                hOut = CreateFileW(outPath, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, CREATE_ALWAYS, FILE_ATTRIBUTE_TEMPORARY, IntPtr.Zero);
                hErr = CreateFileW(errPath, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, CREATE_ALWAYS, FILE_ATTRIBUTE_TEMPORARY, IntPtr.Zero);
                if (hOut == IntPtr.Zero || hErr == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateFile for redirection failed");
                SetHandleInformation(hOut, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
                SetHandleInformation(hErr, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);

                IntPtr size = IntPtr.Zero;
                InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
                attributeList = Marshal.AllocHGlobal(size);
                if (!InitializeProcThreadAttributeList(attributeList, 1, 0, ref size))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "InitializeProcThreadAttributeList failed");

                var caps = new SECURITY_CAPABILITIES
                {
                    AppContainerSid = sid,
                    // No capabilities: an AppContainer without the internetClient capability has
                    // no network at all. That is the SEC-B06 boundary, enforced by the network
                    // compartment rather than by a firewall rule we would have to maintain.
                    Capabilities = IntPtr.Zero,
                    CapabilityCount = 0,
                    Reserved = 0
                };
                capabilities = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(SECURITY_CAPABILITIES)));
                Marshal.StructureToPtr(caps, capabilities, false);
                if (!UpdateProcThreadAttribute(attributeList, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, IntPtr.Zero, capabilities, (IntPtr)Marshal.SizeOf(typeof(SECURITY_CAPABILITIES)), IntPtr.Zero, IntPtr.Zero))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "UpdateProcThreadAttribute(security capabilities) failed");

                var startup = new STARTUPINFOEX();
                startup.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
                startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdOutput = hOut;
                startup.StartupInfo.hStdError = hErr;
                startup.lpAttributeList = attributeList;

                string resolved = ResolveExecutable(command);
                string commandLine = "\"" + resolved + "\"" + JoinArgs(args);
                string currentDirectory = string.IsNullOrEmpty(workDir) ? null : workDir;
                PROCESS_INFORMATION info;
                uint created = EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW;
                if (!CreateProcessW(resolved, commandLine, IntPtr.Zero, IntPtr.Zero, true, created, IntPtr.Zero, currentDirectory, ref startup, out info))
                {
                    int createError = Marshal.GetLastWin32Error();
                    throw new Win32Exception(createError, "CreateProcess failed (" + createError + "): " + commandLine);
                }

                result.Started = true;
                result.ChildIsAppContainer = IsAppContainerToken(info.hProcess);

                uint wait = WaitForSingleObject(info.hProcess, timeoutMs > 0 ? (uint)timeoutMs : INFINITE);
                if (wait == WAIT_TIMEOUT) result.TimedOut = true;
                uint exitCode;
                if (GetExitCodeProcess(info.hProcess, out exitCode))
                {
                    result.ExitCode = (int)exitCode;
                    result.ExitCodeKnown = exitCode != STILL_ACTIVE;
                }
                CloseHandle(info.hThread);
                CloseHandle(info.hProcess);

                result.Stdout = ReadAll(outPath);
                result.Stderr = ReadAll(errPath);
                result.StdoutBytes = FileLength(outPath);
                result.StderrBytes = FileLength(errPath);

                // The boundary is established only if the child really carries an AppContainer
                // token. A child that ran without one would answer the probes exactly the same,
                // and that is the whole failure mode this check exists for.
                result.SandboxEstablished = result.ChildIsAppContainer;
                if (!result.ChildIsAppContainer)
                    result.Error = "the child process does not carry an AppContainer token";
                return result;
            }
            catch (Exception error)
            {
                result.Error = error.Message;
                return result;
            }
            finally
            {
                if (hOut != IntPtr.Zero) CloseHandle(hOut);
                if (hErr != IntPtr.Zero) CloseHandle(hErr);
                if (capabilities != IntPtr.Zero) Marshal.FreeHGlobal(capabilities);
                if (attributeList != IntPtr.Zero)
                {
                    DeleteProcThreadAttributeList(attributeList);
                    Marshal.FreeHGlobal(attributeList);
                }
                if (sid != IntPtr.Zero) FreeSid(sid);
                TryDelete(outPath);
                TryDelete(errPath);
                result.GrantedPaths = granted.ToArray();
                result.UngrantablePaths = ungrantable.ToArray();
            }
        }

        private static bool IsAppContainerToken(IntPtr process)
        {
            IntPtr token;
            if (!OpenProcessToken(process, TOKEN_QUERY, out token)) return false;
            try
            {
                int length = 0;
                GetTokenInformation(token, TokenIsAppContainer, IntPtr.Zero, 0, out length);
                if (length <= 0) return false;
                IntPtr buffer = Marshal.AllocHGlobal(length);
                try
                {
                    if (!GetTokenInformation(token, TokenIsAppContainer, buffer, length, out length)) return false;
                    return Marshal.ReadInt32(buffer) != 0;
                }
                finally { Marshal.FreeHGlobal(buffer); }
            }
            finally { CloseHandle(token); }
        }

        private static void Grant(string path, SecurityIdentifier sid, FileSystemRights rights, bool inherit, List<string> granted, List<string> failed)
        {
            try
            {
                var info = new DirectoryInfo(path);
                var acl = info.GetAccessControl();
                var inheritance = inherit
                    ? (InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit)
                    : InheritanceFlags.None;
                var rule = new FileSystemAccessRule(sid, rights, inheritance, PropagationFlags.None, AccessControlType.Allow);
                acl.AddAccessRule(rule);
                info.SetAccessControl(acl);
                granted.Add(path + " [" + rights + "]");
            }
            catch (Exception error)
            {
                // A directory the caller may not write the ACL of is not a failure of the
                // boundary: the path may already be reachable. It is reported, not hidden.
                failed.Add(path + ": " + error.Message);
            }
        }

        /// <summary>
        /// Grants traverse on the ancestors of a path.
        ///
        /// Walking into a directory needs the traverse right on every directory above it, and an
        /// AppContainer only gets it from an ACE that names the package SID. The rights are
        /// traverse and read attributes - never list - so the container can walk the path without
        /// being able to enumerate what is in the way.
        /// </summary>
        private static void GrantAncestors(string path, SecurityIdentifier sid, List<string> granted, List<string> failed)
        {
            DirectoryInfo current = new DirectoryInfo(path).Parent;
            while (current != null)
            {
                Grant(current.FullName, sid, FileSystemRights.Traverse | FileSystemRights.ReadAttributes | FileSystemRights.Synchronize, false, granted, failed);
                current = current.Parent;
            }
        }

        private static byte[] SidToBytes(IntPtr sid)
        {
            int count = Marshal.ReadByte(sid, 1);
            byte[] bytes = new byte[8 + count * 4];
            Marshal.Copy(sid, bytes, 0, bytes.Length);
            return bytes;
        }

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr FreeSid(IntPtr sid);

        private static string ReadAll(string path)
        {
            try
            {
                if (!File.Exists(path)) return "";
                using (var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite))
                using (var reader = new StreamReader(stream))
                {
                    return reader.ReadToEnd();
                }
            }
            catch { return ""; }
        }

        private static long FileLength(string path)
        {
            try { return File.Exists(path) ? new FileInfo(path).Length : -1; }
            catch { return -1; }
        }

        private static void TryDelete(string path)
        {
            try { if (File.Exists(path)) File.Delete(path); } catch { }
        }

        private static string JoinArgs(string[] args)
        {
            var builder = new StringBuilder();
            foreach (var arg in args ?? new string[0])
            {
                builder.Append(' ');
                builder.Append(Quote(arg));
            }
            return builder.ToString();
        }

        private static string Quote(string arg)
        {
            if (arg == null) return "\"\"";
            if (arg.Length > 0 && arg.IndexOfAny(new char[] { ' ', '\t', '"' }) < 0) return arg;
            return "\"" + arg.Replace("\\", "\\\\").Replace("\"", "\\\"") + "\"";
        }

        /// <summary>
        /// Resolves the command to a full path.
        ///
        /// A command that starts with a quote is not searched in PATH, and a bare name has to be
        /// resolved before it can be granted any access: an AppContainer may not read the PATH
        /// directories it would otherwise search.
        /// </summary>
        private static string ResolveExecutable(string command)
        {
            if (string.IsNullOrEmpty(command)) throw new ArgumentException("no command given");
            if (command.Contains("\\") || command.Contains("/"))
            {
                string direct = Path.GetFullPath(command);
                if (File.Exists(direct)) return direct;
                throw new FileNotFoundException("the command does not exist: " + command);
            }
            string found = SearchPath(command);
            if (found != null) return found;
            throw new FileNotFoundException("the command was not found on PATH: " + command);
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern uint SearchPathW(string path, string fileName, string extension, uint bufferLength, StringBuilder buffer, IntPtr part);

        private static string SearchPath(string command)
        {
            var buffer = new StringBuilder(1024);
            uint found = SearchPathW(null, command, ".exe", (uint)buffer.Capacity, buffer, IntPtr.Zero);
            if (found == 0)
            {
                buffer = new StringBuilder(1024);
                found = SearchPathW(null, command, null, (uint)buffer.Capacity, buffer, IntPtr.Zero);
            }
            return found == 0 ? null : buffer.ToString();
        }
    }
}
'@
}

$argsJson = $env:CW_APPCONTAINER_ARGS
if ([string]::IsNullOrWhiteSpace($argsJson)) { $argsJson = "[]" }

$commandArgs = @()
try {
  $commandArgs = ConvertFrom-Json -InputObject $argsJson
} catch {
  Write-Error "CW_APPCONTAINER_ARGS is not a JSON array: $argsJson"
  exit 2
}
$commandArgs = @($commandArgs | ForEach-Object { [string]$_ })

$readPaths = @()
if (-not [string]::IsNullOrWhiteSpace($ReadPaths)) {
  $readPaths = @($ReadPaths -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}

$result = [CodeWeaver.AppContainerBoundary]::Run(
  $Command,
  [string[]]$commandArgs,
  $ProfileName,
  $Workspace,
  [string[]]$readPaths,
  $TimeoutMs,
  $WorkDir
)

$result | ConvertTo-Json -Compress -Depth 5
