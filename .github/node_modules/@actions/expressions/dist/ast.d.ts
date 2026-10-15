import { ExpressionData } from "./data/index.js";
import { Token } from "./lexer.js";
export interface ExprVisitor<R> {
    visitLiteral(literal: Literal): R;
    visitUnary(unary: Unary): R;
    visitBinary(binary: Binary): R;
    visitLogical(binary: Logical): R;
    visitGrouping(grouping: Grouping): R;
    visitContextAccess(contextAccess: ContextAccess): R;
    visitIndexAccess(indexAccess: IndexAccess): R;
    visitFunctionCall(functionCall: FunctionCall): R;
}
export declare abstract class Expr {
    abstract accept<R>(v: ExprVisitor<R>): R;
}
export declare class Literal extends Expr {
    literal: ExpressionData;
    token: Token;
    constructor(literal: ExpressionData, token: Token);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class Unary extends Expr {
    operator: Token;
    expr: Expr;
    constructor(operator: Token, expr: Expr);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class FunctionCall extends Expr {
    functionName: Token;
    args: Expr[];
    constructor(functionName: Token, args: Expr[]);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class Binary extends Expr {
    left: Expr;
    operator: Token;
    right: Expr;
    constructor(left: Expr, operator: Token, right: Expr);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class Logical extends Expr {
    operator: Token;
    args: Expr[];
    constructor(operator: Token, args: Expr[]);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class Grouping extends Expr {
    group: Expr;
    constructor(group: Expr);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class ContextAccess extends Expr {
    name: Token;
    constructor(name: Token);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class IndexAccess extends Expr {
    expr: Expr;
    index: Expr;
    constructor(expr: Expr, index: Expr);
    accept<R>(v: ExprVisitor<R>): R;
}
export declare class Star extends Expr {
    accept<R>(): R;
}
//# sourceMappingURL=ast.d.ts.map