import { ExpressionDataInterface, Kind } from "./expressiondata.js";
export declare class BooleanData implements ExpressionDataInterface {
    readonly value: boolean;
    constructor(value: boolean);
    readonly kind = Kind.Boolean;
    primitive: boolean;
    coerceString(): string;
    number(): number;
}
//# sourceMappingURL=boolean.d.ts.map