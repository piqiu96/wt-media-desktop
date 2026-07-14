const STATUS_LABELS = Object.freeze({
  idle: "Idle",
  running: "Running",
  starting: "Starting",
  stopped: "Stopped",
  stopping: "Stopping",
  error: "Error",
  unknown: "Unknown",
});

function pendingResultText(count) {
  if (count === 1) {
    return "1 pending result";
  }
  return `${count} pending results`;
}

export function createLocalAgentStatusPage(snapshot) {
  const status = snapshot?.status ?? "unknown";
  const pendingResultCount = Number(snapshot?.pendingResultCount ?? 0);
  const currentTaskId = snapshot?.currentTaskId ?? null;

  return {
    primaryStatus: STATUS_LABELS[status] ?? STATUS_LABELS.unknown,
    agentId: snapshot?.agentId ?? "Not registered",
    currentTaskText: currentTaskId ? `Task ${currentTaskId}` : "No active task",
    pendingResultText: pendingResultText(pendingResultCount),
    actions: [
      {
        id: "start",
        label: "Start",
        enabled: !["running", "starting"].includes(status),
      },
      {
        id: "stop",
        label: "Stop",
        enabled: !["stopped", "stopping", "unknown"].includes(status),
      },
      {
        id: "refresh",
        label: "Refresh",
        enabled: true,
      },
    ],
  };
}
