import { BooleanData } from "../data/index.js";
import { toUpperSpecial } from "../result.js";
export const startswith = {
    name: "startsWith",
    description: "`startsWith( searchString, searchValue )`\n\nReturns `true` when `searchString` starts with `searchValue`. This function is not case sensitive. Casts values to a string.",
    minArgs: 2,
    maxArgs: 2,
    call: (...args) => {
        const left = args[0];
        if (!left.primitive) {
            return new BooleanData(false);
        }
        const right = args[1];
        if (!right.primitive) {
            return new BooleanData(false);
        }
        const ls = toUpperSpecial(left.coerceString());
        const rs = toUpperSpecial(right.coerceString());
        return new BooleanData(ls.startsWith(rs));
    }
};
//# sourceMappingURL=startswith.js.map