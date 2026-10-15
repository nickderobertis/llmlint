import { Kind } from "../data/index.js";
export const caseFunc = {
    name: "case",
    description: "`case( pred1, val1, pred2, val2, ..., default )`\n\nEvaluates predicates in order and returns the value corresponding to the first predicate that evaluates to `true`. If no predicate matches, it returns the last argument as the default value.",
    minArgs: 3,
    maxArgs: Number.MAX_SAFE_INTEGER,
    call: (...args) => {
        // Evaluate predicate-result pairs
        for (let i = 0; i < args.length - 1; i += 2) {
            const predicate = args[i];
            // Predicate must be a boolean
            if (predicate.kind !== Kind.Boolean) {
                throw new Error("case predicate must evaluate to a boolean value");
            }
            // If predicate is true, return the corresponding result
            if (predicate.value) {
                return args[i + 1];
            }
        }
        // No predicate matched, return default (last argument)
        return args[args.length - 1];
    }
};
//# sourceMappingURL=case.js.map