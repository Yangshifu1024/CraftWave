import { AssistantRuntimeProvider, useLocalRuntime } from "@assistant-ui/react";
import { echoAdapter } from "@/chat/runtime";
import { ChatThread } from "@/chat/Thread";

export default function App() {
  const runtime = useLocalRuntime(echoAdapter);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ChatThread />
    </AssistantRuntimeProvider>
  );
}
