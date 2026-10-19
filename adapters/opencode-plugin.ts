import { registerOpenCodePlugin, type OpenCodePluginContext } from "./opencode";

export const OUTRIM_PLUGIN_MARKER = "outrim-managed: opencode-plugin";

export default {
  id: "outrim",
  setup(context: OpenCodePluginContext) {
    return registerOpenCodePlugin(context);
  },
};
