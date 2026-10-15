export declare enum TokenType {
    UNKNOWN = 0,
    LEFT_PAREN = 1,
    RIGHT_PAREN = 2,
    LEFT_BRACKET = 3,
    RIGHT_BRACKET = 4,
    COMMA = 5,
    DOT = 6,
    BANG = 7,
    BANG_EQUAL = 8,
    EQUAL_EQUAL = 9,
    GREATER = 10,
    GREATER_EQUAL = 11,
    LESS = 12,
    LESS_EQUAL = 13,
    AND = 14,
    OR = 15,
    STAR = 16,
    NUMBER = 17,
    STRING = 18,
    IDENTIFIER = 19,
    TRUE = 20,
    FALSE = 21,
    NULL = 22,
    EOF = 23
}
export type Pos = {
    line: number;
    column: number;
};
export type Range = {
    start: Pos;
    end: Pos;
};
export type Token = {
    type: TokenType;
    lexeme: string;
    value?: string | number | boolean;
    range: Range;
};
export declare function tokenString(tok: Token): string;
export type Result = {
    tokens: Token[];
};
export declare class Lexer {
    private input;
    private start;
    private offset;
    private line;
    private lastLineOffset;
    private tokens;
    constructor(input: string);
    lex(): Result;
    private pos;
    private endPos;
    private range;
    private atEnd;
    private peek;
    private peekNext;
    private previous;
    private next;
    private match;
    private addToken;
    private consumeNumber;
    private consumeString;
    private consumeIdentifier;
}
//# sourceMappingURL=lexer.d.ts.map