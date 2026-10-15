import { ExpressionDataInterface, Kind } from "./expressiondata.js";
export declare class NumberData implements ExpressionDataInterface {
    readonly value: number;
    constructor(value: number);
    readonly kind = Kind.Number;
    primitive: boolean;
    coerceString(): string;
    number(): number;
}
//# sourceMappingURL=number.d.ts.map