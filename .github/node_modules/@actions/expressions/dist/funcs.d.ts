import { FunctionDefinition, FunctionInfo } from "./funcs/info.js";
import { Token } from "./lexer.js";
export type ParseContext = {
    allowUnknownKeywords: boolean;
    extensionContexts: Map<string, boolean>;
    extensionFunctions: Map<string, FunctionInfo>;
};
export declare const wellKnownFunctions: {
    [name: string]: FunctionDefinition;
};
export declare function validateFunction(context: ParseContext, identifier: Token, argCount: number): void;
//# sourceMappingURL=funcs.d.ts.map