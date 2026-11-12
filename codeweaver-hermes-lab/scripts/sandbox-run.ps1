<#
.SYNOPSIS
Runs one command inside a Windows job object with resource limits.

.DESCRIPTION
The sandbox that the Phase 0 checklist asked for was a Docker container with --memory, --cpus and
--pids-limit. Docker is gone (doc/HERMES_NATIVE_PLAN.md), but the limits are not a Docker feature:
on Windows they are a job object. This script creates a job, applies the limits, starts the command
suspended, assigns it to the job, and only then lets it run - so the limits are in force before the
first instruction of the child executes.

The result is printed to stdout as one JSON document. The child's own stdout and stderr are
captured into files and returned inside that JSON, because the caller needs both the child's words
and the sandbox's verdict.

.EXAMPLE
powershell -File scripts/sandbox-run.ps1 -Command node -ArgsJson '["-e","console.log(1)"]' -MemoryMb 256
#>
param(
  [Parameter(Mandatory = $true)][string]$Command,
  [string]$ArgsJson = "[]",
  [int]$MemoryMb = 0,
  [int]$CpuRatePercent = 0,
  [int]$ProcessLimit = 0,
  [int]$TimeoutMs = 60000,
  [string]$WorkDir = ""
)

$ErrorActionPreference = 'Stop'

if (-not (Get-Command Add-Type -ErrorAction SilentlyContinue)) {
  Write-Error "Add-Type is unavailable, so the job object helper cannot be compiled"
  exit 2
}

if (-not ('CodeWeaver.JobObject' -as [type])) {
  Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace CodeWeaver
{
    public class SandboxResult
    {
        public bool Started;
        public bool TimedOut;
        public int ExitCode;
        public bool ExitCodeKnown;
        public long PeakJobMemoryBytes;
        public long JobMemoryLimitBytes;
        public int ProcessLimit;
        public string LimitViolation;
        public string Error;
        public string Stdout;
        public string Stderr;
        public long StdoutBytes;
        public long StderrBytes;
    }

    public static class JobObject
    {
        // Job object limit flags (winnt.h, Windows 10 SDK 10.0.26100.0).
        private const uint JOB_OBJECT_LIMIT_ACTIVE_PROCESS = 0x00000008;
        private const uint JOB_OBJECT_LIMIT_PROCESS_MEMORY = 0x00000100;
        private const uint JOB_OBJECT_LIMIT_JOB_MEMORY = 0x00000200;
        private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
        private const uint JOB_OBJECT_LIMIT_RATE_CONTROL = 0x00040000;

        // CPU rate control flags.
        private const uint JOB_OBJECT_CPU_RATE_CONTROL_ENABLE = 0x00000001;
        private const uint JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP = 0x00000004;

        private const int JobObjectExtendedLimitInformation = 9;
        private const int JobObjectCpuRateControlInformation = 15;
        private const int JobObjectLimitViolationInformation2 = 34;

        private const uint CREATE_SUSPENDED = 0x00000004;
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
        private const uint STILL_ACTIVE = 259;

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
        private struct PROCESS_INFORMATION
        {
            public IntPtr hProcess;
            public IntPtr hThread;
            public int dwProcessId;
            public int dwThreadId;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct IO_COUNTERS
        {
            public ulong ReadOperationCount;
            public ulong WriteOperationCount;
            public ulong OtherOperationCount;
            public ulong ReadTransferCount;
            public ulong WriteTransferCount;
            public ulong OtherTransferCount;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_BASIC_LIMIT_INFORMATION
        {
            public long PerProcessUserTimeLimit;
            public long PerJobUserTimeLimit;
            public uint LimitFlags;
            public UIntPtr MinimumWorkingSetSize;
            public UIntPtr MaximumWorkingSetSize;
            public uint ActiveProcessLimit;
            public UIntPtr Affinity;
            public uint PriorityClass;
            public uint SchedulingClass;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
        {
            public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
            public IO_COUNTERS IoInfo;
            public UIntPtr ProcessMemoryLimit;
            public UIntPtr JobMemoryLimit;
            public UIntPtr PeakProcessMemoryUsed;
            public UIntPtr PeakJobMemoryUsed;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_CPU_RATE_CONTROL_INFORMATION
        {
            public uint ControlFlags;
            public uint CpuRate;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_LIMIT_VIOLATION_INFORMATION_2
        {
            public uint JobLimitFlags;
            public uint JobLimitViolationFlags;
            public long JobLimitJobTime;
            public long JobLimitPerProcessUserTime;
            public long JobLimitPerJobUserTime;
            public uint JobLimitFrequency;
            public ulong JobValue;
            public ulong JobRange;
            public IntPtr JobCappingRate;
            public IntPtr JobCappingRate2;
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CreateJobObjectW(IntPtr attributes, string name);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool SetInformationJobObject(IntPtr job, int infoClass, IntPtr info, uint length);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool QueryInformationJobObject(IntPtr job, int infoClass, IntPtr info, uint length, out uint returnLength);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool CreateProcessW(string applicationName, string commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment, string currentDirectory, ref STARTUPINFO startupInfo, out PROCESS_INFORMATION processInformation);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern uint ResumeThread(IntPtr thread);

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

        public static SandboxResult Run(string command, string[] args, int memoryMb, int cpuRatePercent, int processLimit, int timeoutMs, string workDir)
        {
            var result = new SandboxResult { ExitCode = -1, ExitCodeKnown = false, ProcessLimit = processLimit };
            IntPtr job = IntPtr.Zero;
            IntPtr hOut = IntPtr.Zero;
            IntPtr hErr = IntPtr.Zero;
            string outPath = Path.Combine(Path.GetTempPath(), "cw-sandbox-out-" + Guid.NewGuid().ToString("N") + ".txt");
            string errPath = Path.Combine(Path.GetTempPath(), "cw-sandbox-err-" + Guid.NewGuid().ToString("N") + ".txt");
            try
            {
                job = CreateJobObjectW(IntPtr.Zero, null);
                if (job == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateJobObject failed");

                uint flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                var limits = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
                if (memoryMb > 0)
                {
                    flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                    limits.JobMemoryLimit = new UIntPtr((ulong)memoryMb * 1024UL * 1024UL);
                }
                if (processLimit > 0)
                {
                    flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                    limits.BasicLimitInformation.ActiveProcessLimit = (uint)processLimit;
                }
                limits.BasicLimitInformation.LimitFlags = flags;

                int size = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
                IntPtr buffer = Marshal.AllocHGlobal(size);
                try
                {
                    Marshal.StructureToPtr(limits, buffer, false);
                    if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, buffer, (uint)size))
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "SetInformationJobObject(limits) failed");
                }
                finally { Marshal.FreeHGlobal(buffer); }

                if (cpuRatePercent > 0)
                {
                    var cpu = new JOBOBJECT_CPU_RATE_CONTROL_INFORMATION
                    {
                        ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
                        CpuRate = (uint)Math.Max(1, cpuRatePercent) * 100
                    };
                    int cpuSize = Marshal.SizeOf(typeof(JOBOBJECT_CPU_RATE_CONTROL_INFORMATION));
                    IntPtr cpuBuffer = Marshal.AllocHGlobal(cpuSize);
                    try
                    {
                        Marshal.StructureToPtr(cpu, cpuBuffer, false);
                        if (!SetInformationJobObject(job, JobObjectCpuRateControlInformation, cpuBuffer, (uint)cpuSize))
                            throw new Win32Exception(Marshal.GetLastWin32Error(), "SetInformationJobObject(cpu rate) failed");
                    }
                    finally { Marshal.FreeHGlobal(cpuBuffer); }
                }

                hOut = CreateFileW(outPath, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, CREATE_ALWAYS, FILE_ATTRIBUTE_TEMPORARY, IntPtr.Zero);
                hErr = CreateFileW(errPath, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, CREATE_ALWAYS, FILE_ATTRIBUTE_TEMPORARY, IntPtr.Zero);
                if (hOut == IntPtr.Zero || hErr == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "CreateFile for redirection failed");
                SetHandleInformation(hOut, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
                SetHandleInformation(hErr, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);

                var startup = new STARTUPINFO();
                startup.cb = Marshal.SizeOf(typeof(STARTUPINFO));
                startup.dwFlags = STARTF_USESTDHANDLES;
                startup.hStdOutput = hOut;
                startup.hStdError = hErr;

                string commandLine = BuildCommandLine(command, args);
                string resolved = ResolveExecutable(command);
                PROCESS_INFORMATION info;
                uint created = CREATE_SUSPENDED | CREATE_NO_WINDOW;
                string applicationName = null;
                if (IsBatchFile(resolved))
                {
                    // A batch file is not an executable: the shell runs it. The doubled quotes are
                    // the form cmd.exe understands for a command that itself starts with a quote.
                    string cmdExe = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.System), "cmd.exe");
                    string inner = "\"" + resolved + "\"" + JoinArgs(args);
                    commandLine = "\"" + cmdExe + "\" /d /c \"\"" + inner + "\"";
                    applicationName = cmdExe;
                }
                else
                {
                    applicationName = resolved;
                    commandLine = "\"" + resolved + "\"" + JoinArgs(args);
                }
                // An empty working directory is not the current directory: it is an invalid path,
                // and CreateProcess answers ERROR_INVALID_NAME for it.
                string currentDirectory = string.IsNullOrEmpty(workDir) ? null : workDir;
                if (!CreateProcessW(applicationName, commandLine, IntPtr.Zero, IntPtr.Zero, true, created, IntPtr.Zero, currentDirectory, ref startup, out info))
                {
                    int createError = Marshal.GetLastWin32Error();
                    throw new Win32Exception(createError, "CreateProcess failed (" + createError + "): " + commandLine);
                }

                result.Started = true;
                try
                {
                    if (!AssignProcessToJobObject(job, info.hProcess))
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "AssignProcessToJobObject failed");
                }
                finally
                {
                    // The child must run even if the assignment failed: it would otherwise stay
                    // suspended forever, and a suspended child is worse than an unsandboxed one.
                    ResumeThread(info.hThread);
                }

                uint wait = WaitForSingleObject(info.hProcess, timeoutMs > 0 ? (uint)timeoutMs : INFINITE);
                if (wait == WAIT_TIMEOUT)
                {
                    result.TimedOut = true;
                }
                uint exitCode;
                if (GetExitCodeProcess(info.hProcess, out exitCode))
                {
                    result.ExitCode = (int)exitCode;
                    result.ExitCodeKnown = exitCode != STILL_ACTIVE;
                }

                CloseHandle(info.hThread);
                CloseHandle(info.hProcess);

                // The peak numbers come from the job itself: they are what the kernel counted.
                uint returned;
                int querySize = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
                IntPtr queryBuffer = Marshal.AllocHGlobal(querySize);
                try
                {
                    if (QueryInformationJobObject(job, JobObjectExtendedLimitInformation, queryBuffer, (uint)querySize, out returned))
                    {
                        var info2 = (JOBOBJECT_EXTENDED_LIMIT_INFORMATION)Marshal.PtrToStructure(queryBuffer, typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
                        result.PeakJobMemoryBytes = (long)info2.PeakJobMemoryUsed.ToUInt64();
                        result.JobMemoryLimitBytes = (long)info2.JobMemoryLimit.ToUInt64();
                    }
                }
                finally { Marshal.FreeHGlobal(queryBuffer); }

                int violationSize = Marshal.SizeOf(typeof(JOBOBJECT_LIMIT_VIOLATION_INFORMATION_2));
                IntPtr violationBuffer = Marshal.AllocHGlobal(violationSize);
                try
                {
                    if (QueryInformationJobObject(job, JobObjectLimitViolationInformation2, violationBuffer, (uint)violationSize, out returned))
                    {
                        var violation = (JOBOBJECT_LIMIT_VIOLATION_INFORMATION_2)Marshal.PtrToStructure(violationBuffer, typeof(JOBOBJECT_LIMIT_VIOLATION_INFORMATION_2));
                        result.LimitViolation = DescribeViolation(violation.JobLimitFlags);
                    }
                }
                finally { Marshal.FreeHGlobal(violationBuffer); }

                result.Stdout = ReadAll(outPath);
                result.Stderr = ReadAll(errPath);
                result.StdoutBytes = FileLength(outPath);
                result.StderrBytes = FileLength(errPath);
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
                if (job != IntPtr.Zero) CloseHandle(job);
                TryDelete(outPath);
                TryDelete(errPath);
            }
        }

        private static string DescribeViolation(uint flags)
        {
            if (flags == 0) return "";
            var parts = new List<string>();
            if ((flags & JOB_OBJECT_LIMIT_JOB_MEMORY) != 0) parts.Add("job_memory");
            if ((flags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS) != 0) parts.Add("active_process");
            if ((flags & JOB_OBJECT_LIMIT_RATE_CONTROL) != 0) parts.Add("cpu_rate");
            return parts.Count == 0 ? string.Join(",", flags.ToString()) : string.Join(",", parts);
        }

        private static string ReadAll(string path)
        {
            // The parent's own write handle may still be open, and a reader that denies write
            // access cannot open the file next to it: the read would fail with a sharing
            // violation, and a swallowed sharing violation looks exactly like an empty output.
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

        private static string BuildCommandLine(string command, string[] args)
        {
            var builder = new StringBuilder();
            builder.Append('"').Append(command).Append('"');
            builder.Append(JoinArgs(args));
            return builder.ToString();
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

        private static bool IsBatchFile(string path)
        {
            string extension = Path.GetExtension(path ?? "").ToLowerInvariant();
            return extension == ".cmd" || extension == ".bat";
        }

        /// <summary>
        /// The full path of the executable, looked up the way the shell looks it up.
        ///
        /// A quoted first token is not searched for: CreateProcess treats "node" as a literal path
        /// and fails with ERROR_INVALID_NAME, so the path is resolved before the process is made.
        /// </summary>
        private static string ResolveExecutable(string command)
        {
            if (string.IsNullOrEmpty(command)) return command;
            if (command.IndexOfAny(new char[] { '\\', '/' }) >= 0) return command;

            string pathVariable = Environment.GetEnvironmentVariable("PATH") ?? "";
            string extensions = Environment.GetEnvironmentVariable("PATHEXT") ?? ".COM;.EXE;.BAT;.CMD";
            foreach (string directory in pathVariable.Split(';'))
            {
                string dir = directory.Trim();
                if (dir.Length == 0) continue;
                foreach (string extension in extensions.Split(';'))
                {
                    if (extension.Length == 0) continue;
                    string candidate = Path.Combine(dir, command + extension);
                    if (File.Exists(candidate)) return candidate;
                    candidate = Path.Combine(dir, command + extension.ToLowerInvariant());
                    if (File.Exists(candidate)) return candidate;
                }
            }
            return command;
        }

        private static string Quote(string value)
        {
            if (value == null) return "\"\"";
            if (value.Length > 0 && value.IndexOfAny(new char[] { ' ', '\t', '"', '\n', '\v' }) < 0) return value;
            var builder = new StringBuilder();
            builder.Append('"');
            int slashes = 0;
            foreach (char c in value)
            {
                if (c == '\\') { slashes++; continue; }
                if (c == '"')
                {
                    builder.Append('\\', slashes * 2 + 1);
                    builder.Append('"');
                    slashes = 0;
                    continue;
                }
                builder.Append('\\', slashes);
                slashes = 0;
                builder.Append(c);
            }
            builder.Append('\\', slashes * 2);
            builder.Append('"');
            return builder.ToString();
        }
    }
}
'@
}

$argsList = @()
# The arguments arrive as JSON in an environment variable: a command line would have to quote them
# twice, once for the shell and once for PowerShell, and that is exactly where arguments get lost.
# The local is not called $argsJson: PowerShell variable names are case insensitive, so $argsJson
# and the parameter $ArgsJson are one variable, and assigning the local would silently empty the
# parameter before it could be used as the fallback.
$argsJsonValue = $env:CW_SANDBOX_ARGS
if (-not $argsJsonValue) { $argsJsonValue = $ArgsJson }
if ($argsJsonValue -and $argsJsonValue.Trim() -ne "") {
  $parsed = $argsJsonValue | ConvertFrom-Json
  if ($parsed) { $argsList = @($parsed) }
}

$result = [CodeWeaver.JobObject]::Run($Command, [string[]]$argsList, $MemoryMb, $CpuRatePercent, $ProcessLimit, $TimeoutMs, $WorkDir)

# The result is camelCase, because the caller is TypeScript and the keys have to mean the same
# thing on both sides of the boundary.
[ordered]@{
  started = [bool]$result.Started
  timedOut = [bool]$result.TimedOut
  exitCode = [int]$result.ExitCode
  exitCodeKnown = [bool]$result.ExitCodeKnown
  peakJobMemoryBytes = [long]$result.PeakJobMemoryBytes
  jobMemoryLimitBytes = [long]$result.JobMemoryLimitBytes
  processLimit = [int]$result.ProcessLimit
  limitViolation = [string]$result.LimitViolation
  error = [string]$result.Error
  stdout = [string]$result.Stdout
  stderr = [string]$result.Stderr
  stdoutBytes = [long]$result.StdoutBytes
  stderrBytes = [long]$result.StderrBytes
} | ConvertTo-Json -Depth 4 -Compress
