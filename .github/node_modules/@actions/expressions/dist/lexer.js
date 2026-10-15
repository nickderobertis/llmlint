import { StringData } from "./data/index.js";
import { MAX_EXPRESSION_LENGTH } from "./errors.js";
export var TokenType;
(function (TokenType) {
    TokenType[TokenType["UNKNOWN"] = 0] = "UNKNOWN";
    TokenType[TokenType["LEFT_PAREN"] = 1] = "LEFT_PAREN";
    TokenType[TokenType["RIGHT_PAREN"] = 2] = "RIGHT_PAREN";
    TokenType[TokenType["LEFT_BRACKET"] = 3] = "LEFT_BRACKET";
    TokenType[TokenType["RIGHT_BRACKET"] = 4] = "RIGHT_BRACKET";
    TokenType[TokenType["COMMA"] = 5] = "COMMA";
    TokenType[TokenType["DOT"] = 6] = "DOT";
    TokenType[TokenType["BANG"] = 7] = "BANG";
    TokenType[TokenType["BANG_EQUAL"] = 8] = "BANG_EQUAL";
    TokenType[TokenType["EQUAL_EQUAL"] = 9] = "EQUAL_EQUAL";
    TokenType[TokenType["GREATER"] = 10] = "GREATER";
    TokenType[TokenType["GREATER_EQUAL"] = 11] = "GREATER_EQUAL";
    TokenType[TokenType["LESS"] = 12] = "LESS";
    TokenType[TokenType["LESS_EQUAL"] = 13] = "LESS_EQUAL";
    TokenType[TokenType["AND"] = 14] = "AND";
    TokenType[TokenType["OR"] = 15] = "OR";
    TokenType[TokenType["STAR"] = 16] = "STAR";
    TokenType[TokenType["NUMBER"] = 17] = "NUMBER";
    TokenType[TokenType["STRING"] = 18] = "STRING";
    TokenType[TokenType["IDENTIFIER"] = 19] = "IDENTIFIER";
    TokenType[TokenType["TRUE"] = 20] = "TRUE";
    TokenType[TokenType["FALSE"] = 21] = "FALSE";
    TokenType[TokenType["NULL"] = 22] = "NULL";
    TokenType[TokenType["EOF"] = 23] = "EOF";
})(TokenType || (TokenType = {}));
export function tokenString(tok) {
    switch (tok.type) {
        case TokenType.EOF:
            return "EOF";
        case TokenType.NUMBER:
            return tok.lexeme;
        case TokenType.STRING:
            // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
            return tok.value.toString();
        default:
            return tok.lexeme;
    }
}
export class Lexer {
    constructor(input) {
        this.input = input;
        this.start = 0;
        this.offset = 0;
        this.line = 0;
        this.lastLineOffset = 0;
        this.tokens = [];
    }
    lex() {
        if (this.input.length > MAX_EXPRESSION_LENGTH) {
            throw new Error("ErrorExceededMaxLength");
        }
        while (!this.atEnd()) {
            this.start = this.offset;
            const c = this.next();
            switch (c) {
                case "(":
                    this.addToken(TokenType.LEFT_PAREN);
                    break;
                case ")":
                    this.addToken(TokenType.RIGHT_PAREN);
                    break;
                case "[":
                    this.addToken(TokenType.LEFT_BRACKET);
                    break;
                case "]":
                    this.addToken(TokenType.RIGHT_BRACKET);
                    break;
                case ",":
                    this.addToken(TokenType.COMMA);
                    break;
                case ".":
                    if (this.previous() != TokenType.IDENTIFIER &&
                        this.previous() != TokenType.RIGHT_BRACKET &&
                        this.previous() != TokenType.RIGHT_PAREN &&
                        this.previous() != TokenType.STAR) {
                        this.consumeNumber();
                    }
                    else {
                        this.addToken(TokenType.DOT);
                    }
                    break;
                case "-":
                case "+":
                    this.consumeNumber();
                    break;
                case "!":
                    this.addToken(this.match("=") ? TokenType.BANG_EQUAL : TokenType.BANG);
                    break;
                case "=":
                    if (!this.match("=")) {
                        // Illegal; continue reading until we hit a boundary character and return an error
                        this.consumeIdentifier();
                        break;
                    }
                    this.addToken(TokenType.EQUAL_EQUAL);
                    break;
                case "<":
                    this.addToken(this.match("=") ? TokenType.LESS_EQUAL : TokenType.LESS);
                    break;
                case ">":
                    this.addToken(this.match("=") ? TokenType.GREATER_EQUAL : TokenType.GREATER);
                    break;
                case "&":
                    if (!this.match("&")) {
                        // Illegal; continue reading until we hit a boundary character and return an error
                        this.consumeIdentifier();
                        break;
                    }
                    this.addToken(TokenType.AND);
                    break;
                case "|":
                    if (!this.match("|")) {
                        // Illegal; continue reading until we hit a boundary character and return an error
                        this.consumeIdentifier();
                        break;
                    }
                    this.addToken(TokenType.OR);
                    break;
                case "*":
                    this.addToken(TokenType.STAR);
                    break;
                // Ignore whitespace.
                case " ":
                case "\r":
                case "\t":
                    break;
                case "\n":
                    ++this.line;
                    this.lastLineOffset = this.offset;
                    break;
                case "'":
                    this.consumeString();
                    break;
                default:
                    switch (true) {
                        case isDigit(c):
                            this.consumeNumber();
                            break;
                        default:
                            this.consumeIdentifier();
                            break;
                    }
            }
        }
        this.tokens.push({
            type: TokenType.EOF,
            lexeme: "",
            range: this.range()
        });
        return {
            tokens: this.tokens
        };
    }
    pos() {
        return {
            line: this.line,
            column: this.start - this.lastLineOffset
        };
    }
    endPos() {
        return {
            line: this.line,
            column: this.offset - this.lastLineOffset
        };
    }
    range() {
        return {
            start: this.pos(),
            end: this.endPos()
        };
    }
    atEnd() {
        return this.offset >= this.input.length;
    }
    peek() {
        if (this.atEnd()) {
            return "\0";
        }
        return this.input[this.offset];
    }
    peekNext() {
        if (this.offset + 1 >= this.input.length) {
            return "\0";
        }
        return this.input[this.offset + 1];
    }
    previous() {
        const l = this.tokens.length;
        if (l == 0) {
            return TokenType.EOF;
        }
        return this.tokens[l - 1].type;
    }
    next() {
        return this.input[this.offset++];
    }
    match(expected) {
        if (this.atEnd()) {
            return false;
        }
        if (this.input[this.offset] !== expected) {
            return false;
        }
        this.offset++;
        return true;
    }
    addToken(type, value) {
        this.tokens.push({
            type,
            lexeme: this.input.substring(this.start, this.offset),
            range: this.range(),
            value
        });
    }
    consumeNumber() {
        while (!this.atEnd() && (!isBoundary(this.peek()) || this.peek() == ".")) {
            this.next();
        }
        const lexeme = this.input.substring(this.start, this.offset);
        const value = new StringData(lexeme).number();
        if (isNaN(value)) {
            throw new Error(`Unexpected symbol: '${lexeme}'. Located at position ${this.start + 1} within expression: ${this.input}`);
        }
        this.addToken(TokenType.NUMBER, value);
    }
    consumeString() {
        while ((this.peek() !== "'" || this.peekNext() === "'") && !this.atEnd()) {
            if (this.peek() === "\n")
                this.line++;
            if (this.peek() === "'" && this.peekNext() === "'") {
                // Escaped "'", consume
                this.next();
            }
            this.next();
        }
        if (this.atEnd()) {
            // Unterminated string
            throw new Error(`Unexpected symbol: '${this.input.substring(this.start)}'. Located at position ${this.start + 1} within expression: ${this.input}`);
        }
        // Closing '
        this.next();
        // Trim the surrounding quotes.
        let value = this.input.substring(this.start + 1, this.offset - 1);
        value = value.replace("''", "'");
        this.addToken(TokenType.STRING, value);
    }
    consumeIdentifier() {
        while (!this.atEnd() && !isBoundary(this.peek())) {
            this.next();
        }
        let tokenType = TokenType.IDENTIFIER;
        let tokenValue = undefined;
        const lexeme = this.input.substring(this.start, this.offset);
        if (this.previous() != TokenType.DOT) {
            switch (lexeme) {
                case "true":
                    tokenType = TokenType.TRUE;
                    break;
                case "false":
                    tokenType = TokenType.FALSE;
                    break;
                case "null":
                    tokenType = TokenType.NULL;
                    break;
                case "NaN":
                    tokenType = TokenType.NUMBER;
                    tokenValue = NaN;
                    break;
                case "Infinity":
                    tokenType = TokenType.NUMBER;
                    tokenValue = Infinity;
                    break;
            }
        }
        if (!isLegalIdentifier(lexeme)) {
            throw new Error(`Unexpected symbol: '${lexeme}'. Located at position ${this.start + 1} within expression: ${this.input}`);
        }
        this.addToken(tokenType, tokenValue);
    }
}
function isDigit(c) {
    return c >= "0" && c <= "9";
}
function isBoundary(c) {
    switch (c) {
        case "(":
        case "[":
        case ")":
        case "]":
        case ",":
        case ".":
        case "!":
        case ">":
        case "<":
        case "=":
        case "&":
        case "|":
            return true;
    }
    return /\s/.test(c);
}
function isLegalIdentifier(str) {
    if (str == "") {
        return false;
    }
    const first = str[0];
    if ((first >= "a" && first <= "z") || (first >= "A" && first <= "Z") || first == "_") {
        for (const c of str.substring(1).split("")) {
            if ((c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9") || c == "_" || c == "-") {
                // OK
            }
            else {
                return false;
            }
        }
        return true;
    }
    return false;
}
//# sourceMappingURL=lexer.js.map