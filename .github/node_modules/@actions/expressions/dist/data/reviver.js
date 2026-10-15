import { Array as dArray } from "./array.js";
import { BooleanData } from "./boolean.js";
import { Dictionary } from "./dictionary.js";
import { Null } from "./null.js";
import { NumberData } from "./number.js";
import { StringData } from "./string.js";
/**
 * Reviver can be passed to `JSON.parse` to convert plain JSON into an `ExpressionData` object.
 *
 * See: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/JSON/parse#reviver
 */
export function reviver(_key, val) {
    if (val === null) {
        return new Null();
    }
    if (typeof val === "string") {
        return new StringData(val);
    }
    if (typeof val === "number") {
        return new NumberData(val);
    }
    if (typeof val === "boolean") {
        return new BooleanData(val);
    }
    if (Array.isArray(val)) {
        return new dArray(...val);
    }
    if (typeof val === "object") {
        return new Dictionary(...Object.keys(val).map(k => ({
            key: k,
            value: val[k]
        })));
    }
    // Pass through value
    return val;
}
//# sourceMappingURL=reviver.js.map