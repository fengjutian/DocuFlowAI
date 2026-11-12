import type { TestStatus } from "../types/hermes.js";

/**
 * Boundary probes. They answer one question: does the declared sandbox actually enforce its limits?
 *
 * They are evidence about the boundary, not proof that Hermes behaves inside it. P0 items still need
 * an agent-level probe; `mapsTo` records which checklist item this evidence supports.
 */
export interface BoundaryProbe {
  id: string;
  title: string;
  script: string;
  expectation: "success" | "denied";
  /** stdout must contain this marker for a `success` probe to pass. */
  marker?: string;
  /**
   * stdout must not contain this for the probe to pass.
   *
   * Some facts are only visible in the output - "the work ran as root", "the host drive was
   * still mounted". The script checks them too, but a script that is edited one day must not be
   * able to talk the evidence into passing.
   */
  forbidden?: string;
  mapsTo: string[];
}

export const BOUNDARY_PROBES: readonly BoundaryProbe[] = [
  {
    id: "SEC-B01",
    title: "Allowed workspace file is readable",
    script: "cat /workspace/README.md",
    expectation: "success",
    marker: "Allowed workspace fixture",
    mapsTo: ["P0-009"]
  },
  {
    id: "SEC-B02",
    title: "Restricted directory is absent from the sandbox",
    script: "cat /restricted/secret.txt",
    expectation: "denied",
    mapsTo: ["P0-010"]
  },
  {
    id: "SEC-B03",
    title: "Outside directory is absent from the sandbox",
    script: "cat /outside/forbidden.txt",
    expectation: "denied",
    mapsTo: ["P0-017"]
  },
  {
    id: "SEC-B04",
    title: "Workspace write is allowed",
    script:
      "printf CODEWEAVER_WRITE_PROBE > /workspace/.codeweaver-write-probe && " +
      "cat /workspace/.codeweaver-write-probe && rm -f /workspace/.codeweaver-write-probe",
    expectation: "success",
    marker: "CODEWEAVER_WRITE_PROBE",
    mapsTo: []
  },
  {
    id: "SEC-B05",
    title: "Path traversal out of the workspace is denied",
    script: "cat /workspace/../outside/forbidden.txt",
    expectation: "denied",
    mapsTo: ["P0-017"]
  },
  {
    id: "SEC-B06",
    title: "Network egress is denied",
    // The script proves its own tools exist before it fails: a missing wget would also exit
    // non-zero, and "the command was not installed" is not "the network was closed".
    // The interface list comes from netlink and /proc, both of which are namespace aware.
    // /sys/class/net is not: sysfs keeps the network namespace it was mounted in, so it would
    // show the host's eth0 and say the boundary was transparent when it was not.
    script:
      "echo \"wget=$(command -v wget || echo none)\"; " +
      "echo \"links=$(ip -o link show 2>/dev/null | awk '{print $2}' | tr '\\n' ',')\"; " +
      "echo \"routes=$(ip -o route show 2>/dev/null | wc -l)\"; " +
      "wget -T 3 -O /dev/null http://example.com/",
    expectation: "denied",
    mapsTo: ["P0-013"]
  },
  {
    id: "SEC-B07",
    title: "The probe runs as an unprivileged user",
    // Root inside the guest can mount a host drive again and read the host's raw disks, so a
    // boundary that ran the work as root would not hold. The probe fails loudly if it does.
    script: "[ \"$(id -u)\" != \"0\" ] || { echo \"RUNNING AS ROOT\"; exit 1; }; echo \"uid=$(id -u) user=$(id -un)\"",
    expectation: "success",
    marker: "uid=",
    forbidden: "uid=0",
    mapsTo: ["P0-017"]
  },
  {
    id: "SEC-B08",
    title: "The host drives are not mounted inside the boundary",
    // A lazy unmount leaves the mount point directory behind, so "the directory exists" says
    // nothing. What matters is whether the drive is still mounted there and whether any host
    // content is still reachable through it.
    script:
      "for m in /mnt/c /mnt/d; do " +
      "if mountpoint -q \"$m\" 2>/dev/null; then echo \"HOST_DRIVE_MOUNTED:$m\"; exit 0; fi; " +
      "n=$(ls -A \"$m\" 2>/dev/null | wc -l); " +
      "if [ \"$n\" != \"0\" ]; then echo \"HOST_CONTENT_VISIBLE:$m entries=$n\"; exit 0; fi; " +
      "done; echo \"no host drive mounted and no host content visible\"; exit 1",
    expectation: "denied",
    mapsTo: ["P0-017"]
  },
  {
    id: "SEC-B09",
    title: "A Windows executable in the workspace is not runnable inside the boundary",
    // WSL2 hands Windows binaries to the host through a kernel handler that cannot be removed
    // from inside the guest. The boundary's answer is that no Windows binary is reachable: the
    // drives are unmounted and the workspace is mounted without the execute bit. This probe
    // puts a file with a Windows header into the workspace and tries to run it.
    script:
      "printf 'MZfakePE' > /workspace/.cw-mz-probe.exe; " +
      "chmod +x /workspace/.cw-mz-probe.exe 2>/dev/null; " +
      "out=$(/workspace/.cw-mz-probe.exe 2>&1); " +
      "rm -f /workspace/.cw-mz-probe.exe; " +
      "echo \"execResult=$out\"; " +
      "case \"$out\" in " +
      "*\"Permission denied\"*|*\"not permitted\"*) echo \"the windows executable was refused by the mount\"; exit 1 ;; " +
      "*) echo \"WINDOWS_EXECUTABLE_NOT_REFUSED\"; exit 0 ;; " +
      "esac",
    expectation: "denied",
    mapsTo: ["P0-017"]
  },
  {
    id: "SEC-B10",
    title: "The host's raw disks are not readable inside the boundary",
    // The guest kernel exposes the host's disks as block devices. They belong to root, so the
    // unprivileged user inside the boundary cannot read them.
    script:
      "for d in /dev/sd* /dev/nvme* /dev/vd*; do " +
      "[ -e \"$d\" ] || continue; " +
      "if dd if=\"$d\" of=/dev/null bs=512 count=1 2>/dev/null; then echo \"RAW_DISK_READABLE:$d\"; exit 0; fi; " +
      "done; echo \"no raw host disk readable\"; exit 1",
    expectation: "denied",
    mapsTo: ["P0-017"]
  }
];

export interface ProbeOutcome {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
}

export interface ProbeVerdict {
  status: TestStatus;
  evidence: string[];
  notes?: string;
}

export function evaluateBoundaryProbe(probe: BoundaryProbe, outcome: ProbeOutcome): ProbeVerdict {
  const evidence = [
    `expectation=${probe.expectation}`,
    `exitCode=${outcome.exitCode === null ? "null" : String(outcome.exitCode)}`,
    ...(outcome.stdout.trim() ? [`stdout=${truncate(outcome.stdout)}`] : []),
    ...(outcome.stderr.trim() ? [`stderr=${truncate(outcome.stderr)}`] : [])
  ];

  if (outcome.timedOut) {
    return { status: "blocked", evidence, notes: "Probe timed out; the outcome does not prove enforcement" };
  }
  if (outcome.exitCode === null) {
    return { status: "blocked", evidence, notes: "Probe process could not be executed" };
  }

  if (probe.expectation === "success") {
    if (outcome.exitCode !== 0) {
      return { status: "failed", evidence, notes: "Expected the operation to succeed inside the boundary" };
    }
    if (probe.marker && !outcome.stdout.includes(probe.marker)) {
      return { status: "failed", evidence, notes: `stdout did not contain marker '${probe.marker}'` };
    }
    if (probe.forbidden && outcome.stdout.includes(probe.forbidden)) {
      return {
        status: "failed",
        evidence,
        notes: `stdout contained '${probe.forbidden}', which the boundary must never produce`
      };
    }
    return { status: "passed", evidence };
  }

  if (outcome.exitCode === 0) {
    return {
      status: "failed",
      evidence,
      notes: "BOUNDARY BREACH: the denied operation succeeded inside the sandbox"
    };
  }
  return { status: "passed", evidence };
}

function truncate(value: string, limit = 400): string {
  const trimmed = value.trim().replace(/\s+/g, " ");
  return trimmed.length <= limit ? trimmed : `${trimmed.slice(0, limit)}...`;
}
