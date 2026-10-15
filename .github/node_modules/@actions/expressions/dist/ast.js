export class Expr {
}
export class Literal extends Expr {
    constructor(literal, token) {
        super();
        this.literal = literal;
        this.token = token;
    }
    accept(v) {
        return v.visitLiteral(this);
    }
}
export class Unary extends Expr {
    constructor(operator, expr) {
        super();
        this.operator = operator;
        this.expr = expr;
    }
    accept(v) {
        return v.visitUnary(this);
    }
}
export class FunctionCall extends Expr {
    constructor(functionName, args) {
        super();
        this.functionName = functionName;
        this.args = args;
    }
    accept(v) {
        return v.visitFunctionCall(this);
    }
}
export class Binary extends Expr {
    constructor(left, operator, right) {
        super();
        this.left = left;
        this.operator = operator;
        this.right = right;
    }
    accept(v) {
        return v.visitBinary(this);
    }
}
export class Logical extends Expr {
    constructor(operator, args) {
        super();
        this.operator = operator;
        this.args = args;
    }
    accept(v) {
        return v.visitLogical(this);
    }
}
export class Grouping extends Expr {
    constructor(group) {
        super();
        this.group = group;
    }
    accept(v) {
        return v.visitGrouping(this);
    }
}
export class ContextAccess extends Expr {
    constructor(name) {
        super();
        this.name = name;
    }
    accept(v) {
        return v.visitContextAccess(this);
    }
}
export class IndexAccess extends Expr {
    constructor(expr, index) {
        super();
        this.expr = expr;
        this.index = index;
    }
    accept(v) {
        return v.visitIndexAccess(this);
    }
}
export class Star extends Expr {
    accept() {
        throw new Error("Method not implemented.");
    }
}
//# sourceMappingURL=ast.js.map