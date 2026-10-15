import { Expr } from "./ast.js";
import { FunctionInfo } from "./funcs/info.js";
import { Token } from "./lexer.js";
export declare class Parser {
    private tokens;
    private extContexts;
    private extFuncs;
    private offset;
    private depth;
    private context;
    /**
     * Constructs a new parser for the given tokens
     *
     * @param tokens Tokens to build a parse tree from
     * @param extensionContexts Available context names
     * @param extensionFunctions Available functions (beyond the built-in ones)
     */
    constructor(tokens: Token[], extensionContexts: string[], extensionFunctions: FunctionInfo[]);
    parse(): Expr;
    private expression;
    private logicalOr;
    private logicalAnd;
    private equality;
    private comparison;
    private unary;
    private index;
    private call;
    private primary;
    private match;
    private check;
    private atEnd;
    private next;
    private peek;
    private previous;
    private consume;
    private incrDepth;
    private decrDepth;
    private buildError;
}
//# sourceMappingURL=parser.d.ts.map