import { Binary, ContextAccess, Expr, ExprVisitor, FunctionCall, Grouping, IndexAccess, Literal, Logical, Unary } from "./ast.js";
import * as data from "./data/index.js";
import { FunctionDefinition } from "./funcs/info.js";
export declare class Evaluator implements ExprVisitor<data.ExpressionData> {
    private n;
    private context;
    private functions?;
    /**
     * Creates a new evaluator
     * @param n Parsed expression to evaluate
     * @param context Context data to use
     * @param functions Optional map of function implementations. If given, these will be preferred over the built-in functions.
     */
    constructor(n: Expr, context: data.Dictionary, functions?: Map<string, FunctionDefinition> | undefined);
    evaluate(): data.ExpressionData;
    protected eval(n: Expr): data.ExpressionData;
    visitLiteral(literal: Literal): data.ExpressionData;
    visitUnary(unary: Unary): data.ExpressionData;
    visitBinary(binary: Binary): data.ExpressionData;
    visitLogical(logical: Logical): data.ExpressionData;
    visitGrouping(grouping: Grouping): data.ExpressionData;
    visitContextAccess(contextAccess: ContextAccess): data.ExpressionData;
    visitIndexAccess(ia: IndexAccess): data.ExpressionData;
    visitFunctionCall(functionCall: FunctionCall): data.ExpressionData;
}
//# sourceMappingURL=evaluator.d.ts.map