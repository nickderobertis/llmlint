import { Star } from "./ast.js";
import * as data from "./data/index.js";
import { FilteredArray } from "./filtered_array.js";
import { wellKnownFunctions } from "./funcs.js";
import { idxHelper } from "./idxHelper.js";
import { TokenType } from "./lexer.js";
import { equals, falsy, greaterThan, lessThan, truthy } from "./result.js";
export class Evaluator {
    /**
     * Creates a new evaluator
     * @param n Parsed expression to evaluate
     * @param context Context data to use
     * @param functions Optional map of function implementations. If given, these will be preferred over the built-in functions.
     */
    constructor(n, context, functions) {
        this.n = n;
        this.context = context;
        this.functions = functions;
    }
    evaluate() {
        return this.eval(this.n);
    }
    eval(n) {
        return n.accept(this);
    }
    visitLiteral(literal) {
        return literal.literal;
    }
    visitUnary(unary) {
        const r = this.eval(unary.expr);
        if (unary.operator.type === TokenType.BANG) {
            return new data.BooleanData(falsy(r));
        }
        throw new Error(`unknown unary operator: ${unary.operator.lexeme}`);
    }
    visitBinary(binary) {
        const left = this.eval(binary.left);
        const right = this.eval(binary.right);
        switch (binary.operator.type) {
            case TokenType.EQUAL_EQUAL:
                return new data.BooleanData(equals(left, right));
            case TokenType.BANG_EQUAL:
                return new data.BooleanData(!equals(left, right));
            case TokenType.GREATER:
                return new data.BooleanData(greaterThan(left, right));
            case TokenType.GREATER_EQUAL:
                return new data.BooleanData(equals(left, right) || greaterThan(left, right));
            case TokenType.LESS:
                return new data.BooleanData(lessThan(left, right));
            case TokenType.LESS_EQUAL:
                return new data.BooleanData(equals(left, right) || lessThan(left, right));
        }
        throw new Error(`unknown binary operator: ${binary.operator.lexeme}`);
    }
    visitLogical(logical) {
        let result;
        for (const arg of logical.args) {
            result = this.eval(arg);
            // Break?
            if ((logical.operator.type === TokenType.AND && falsy(result)) ||
                (logical.operator.type === TokenType.OR && truthy(result))) {
                break;
            }
        }
        // result is always assigned before we return here
        // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
        return result;
    }
    visitGrouping(grouping) {
        return this.eval(grouping.group);
    }
    visitContextAccess(contextAccess) {
        // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
        const r = this.context.get(contextAccess.name.lexeme);
        return r;
    }
    visitIndexAccess(ia) {
        let idx;
        if (ia.index instanceof Star) {
            idx = new idxHelper(true, undefined);
        }
        else {
            let idxResult;
            try {
                idxResult = this.eval(ia.index);
            }
            catch (e) {
                throw new Error(`could not evaluate index for index access: ${e.message}`, { cause: e });
            }
            idx = new idxHelper(false, idxResult);
        }
        const objResult = this.eval(ia.expr);
        let result;
        switch (objResult.kind) {
            case data.Kind.Array: {
                const tobjResult = objResult;
                if (tobjResult instanceof FilteredArray) {
                    result = filteredArrayAccess(tobjResult, idx);
                }
                else {
                    result = arrayAccess(tobjResult, idx);
                }
                break;
            }
            case data.Kind.Dictionary: {
                const tobjResult = objResult;
                result = objectAccess(tobjResult, idx);
                break;
            }
            default:
                if (idx.star) {
                    result = new FilteredArray();
                }
                else {
                    result = new data.Null();
                }
        }
        return result;
    }
    visitFunctionCall(functionCall) {
        // Evaluate arguments
        const args = functionCall.args.map(arg => this.eval(arg));
        // Get function definitions
        const functionName = functionCall.functionName.lexeme.toLowerCase();
        const f = this.functions?.get(functionName) || wellKnownFunctions[functionName];
        return f.call(...args);
    }
}
function filteredArrayAccess(fa, idx) {
    const result = new FilteredArray();
    for (const item of fa.values()) {
        // Check the type of the nested item
        switch (item.kind) {
            case data.Kind.Dictionary: {
                const ti = item;
                if (idx.star) {
                    for (const v of ti.values()) {
                        result.add(v);
                    }
                }
                else if (idx.str !== undefined) {
                    const v = ti.get(idx.str);
                    if (v !== undefined) {
                        result.add(v);
                    }
                }
                break;
            }
            case data.Kind.Array: {
                const ti = item;
                if (idx.star) {
                    for (const v of ti.values()) {
                        result.add(v);
                    }
                }
                else if (idx.int !== undefined && idx.int < ti.values().length) {
                    result.add(ti.get(idx.int));
                }
                break;
            }
        }
    }
    return result;
}
function arrayAccess(a, idx) {
    if (idx.star) {
        const fa = new FilteredArray();
        for (const item of a.values()) {
            fa.add(item);
        }
        return fa;
    }
    if (idx.int !== undefined && idx.int < a.values().length) {
        return a.get(idx.int);
    }
    return new data.Null();
}
function objectAccess(obj, idx) {
    if (idx.star) {
        const fa = new FilteredArray(...obj.values());
        return fa;
    }
    if (idx.str !== undefined) {
        const r = obj.get(idx.str);
        if (r !== undefined) {
            return r;
        }
    }
    return new data.Null();
}
//# sourceMappingURL=evaluator.js.map