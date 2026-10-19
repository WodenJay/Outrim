import { type OpenCodePluginContext, registerOpenCodePlugin } from "./opencode";

export const OUTRIM_PLUGIN_MARKER = "outrim-managed: opencode-plugin";

export default {
  id: "outrim",
  setup(context: OpenCodePluginContext) {
    return registerOpenCodePlugin(context);
  },
};
