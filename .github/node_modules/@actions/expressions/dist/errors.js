import { tokenString } from "./lexer.js";
export const MAX_PARSER_DEPTH = 50;
export const MAX_EXPRESSION_LENGTH = 21000;
export var ErrorType;
(function (ErrorType) {
    ErrorType[ErrorType["ErrorUnexpectedSymbol"] = 0] = "ErrorUnexpectedSymbol";
    ErrorType[ErrorType["ErrorUnrecognizedNamedValue"] = 1] = "ErrorUnrecognizedNamedValue";
    ErrorType[ErrorType["ErrorUnexpectedEndOfExpression"] = 2] = "ErrorUnexpectedEndOfExpression";
    ErrorType[ErrorType["ErrorExceededMaxDepth"] = 3] = "ErrorExceededMaxDepth";
    ErrorType[ErrorType["ErrorExceededMaxLength"] = 4] = "ErrorExceededMaxLength";
    ErrorType[ErrorType["ErrorTooFewParameters"] = 5] = "ErrorTooFewParameters";
    ErrorType[ErrorType["ErrorTooManyParameters"] = 6] = "ErrorTooManyParameters";
    ErrorType[ErrorType["ErrorEvenParameters"] = 7] = "ErrorEvenParameters";
    ErrorType[ErrorType["ErrorUnrecognizedContext"] = 8] = "ErrorUnrecognizedContext";
    ErrorType[ErrorType["ErrorUnrecognizedFunction"] = 9] = "ErrorUnrecognizedFunction";
})(ErrorType || (ErrorType = {}));
export class ExpressionError extends Error {
    constructor(typ, tok) {
        super(`${errorDescription(typ)}: '${tokenString(tok)}'`);
        this.typ = typ;
        this.tok = tok;
        this.pos = this.tok.range.start;
    }
}
function errorDescription(typ) {
    switch (typ) {
        case ErrorType.ErrorUnexpectedEndOfExpression:
            return "Unexpected end of expression";
        case ErrorType.ErrorUnexpectedSymbol:
            return "Unexpected symbol";
        case ErrorType.ErrorUnrecognizedNamedValue:
            return "Unrecognized named-value";
        case ErrorType.ErrorExceededMaxDepth:
            return `Exceeded max expression depth ${MAX_PARSER_DEPTH}`;
        case ErrorType.ErrorExceededMaxLength:
            return `Exceeded max expression length ${MAX_EXPRESSION_LENGTH}`;
        case ErrorType.ErrorTooFewParameters:
            return "Too few parameters supplied";
        case ErrorType.ErrorTooManyParameters:
            return "Too many parameters supplied";
        case ErrorType.ErrorEvenParameters:
            return "Even number of parameters supplied, requires an odd number of parameters";
        case ErrorType.ErrorUnrecognizedContext:
            return "Unrecognized named-value";
        case ErrorType.ErrorUnrecognizedFunction:
            return "Unrecognized function";
        default: // Should never reach here.
            return "Unknown error";
    }
}
export class ExpressionEvaluationError extends Error {
}
//# sourceMappingURL=errors.js.map