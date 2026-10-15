import { StringData } from "../data/index.js";
import { replacer } from "../data/replacer.js";
export const tojson = {
    name: "toJson",
    description: "`toJSON(value)`\n\nReturns a pretty-print JSON representation of `value`. You can use this function to debug the information provided in contexts.",
    minArgs: 1,
    maxArgs: 1,
    call: (...args) => {
        return new StringData(JSON.stringify(args[0], replacer, "  "));
    }
};
//# sourceMappingURL=tojson.js.map