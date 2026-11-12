import { useEffect, useState } from "react";
import { Navigate, Route, Routes } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { AppShell } from "@/components/layout/AppShell";
import { SettingsDialog } from "@/components/dialogs/SettingsDialog";
import { NewTaskDialog } from "@/components/dialogs/NewTaskDialog";
import { ExecutionStreamProvider } from "@/state/ExecutionStreamProvider";
import { seedFromRuntimeDefaults } from "@/lib/settings";
import { OverviewPage } from "@/pages/OverviewPage";
import { RuntimesPage } from "@/pages/RuntimesPage";
import { ExecutionsPage } from "@/pages/ExecutionsPage";
import { ExecutionDetailPage } from "@/pages/ExecutionDetailPage";
import { SkillsPage } from "@/pages/SkillsPage";
import { SubagentsPage } from "@/pages/SubagentsPage";
import { UsagePage } from "@/pages/UsagePage";
import { TracePage } from "@/pages/TracePage";
import { AuditTimelinePage } from "@/pages/AuditTimelinePage";

export default function App() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [newTaskOpen, setNewTaskOpen] = useState(false);

  useEffect(() => {
    void seedFromRuntimeDefaults();
  }, []);

  return (
    <TooltipProvider delayDuration={200}>
      <ExecutionStreamProvider>
        <AppShell onOpenSettings={() => setSettingsOpen(true)}>
          <Routes>
            <Route path="/" element={<OverviewPage onNewTask={() => setNewTaskOpen(true)} />} />
            <Route path="/runtimes" element={<RuntimesPage />} />
            <Route path="/executions" element={<ExecutionsPage onNewTask={() => setNewTaskOpen(true)} />} />
            <Route path="/executions/:executionId" element={<ExecutionDetailPage />} />
            <Route path="/skills" element={<SkillsPage />} />
            <Route path="/subagents" element={<SubagentsPage />} />
            <Route path="/usage" element={<UsagePage />} />
            <Route path="/trace" element={<TracePage />} />
            <Route path="/audit-timeline" element={<AuditTimelinePage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </AppShell>

        <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
        <NewTaskDialog open={newTaskOpen} onOpenChange={setNewTaskOpen} />

        <Toaster position="bottom-right" richColors />
      </ExecutionStreamProvider>
    </TooltipProvider>
  );
}
