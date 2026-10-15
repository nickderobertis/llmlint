import { Kind, StringData } from "../data/index.js";
export const join = {
    name: "join",
    description: "`join( array, optionalSeparator )`\n\nThe value for `array` can be an array or a string. All values in `array` are concatenated into a string. If you provide `optionalSeparator`, it is inserted between the concatenated values. Otherwise, the default separator `,` is used. Casts values to a string.",
    minArgs: 1,
    maxArgs: 2,
    call: (...args) => {
        // Primitive
        if (args[0].primitive) {
            return new StringData(args[0].coerceString());
        }
        // Array
        if (args[0].kind === Kind.Array) {
            // Separator
            let separator = ",";
            if (args.length > 1 && args[1].primitive) {
                separator = args[1].coerceString();
            }
            // Convert items to strings
            return new StringData(args[0]
                .values()
                .map(item => item.coerceString())
                .join(separator));
        }
        return new StringData("");
    }
};
//# sourceMappingURL=join.js.map