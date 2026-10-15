import { ExpressionDataInterface, Kind } from "./expressiondata.js";
export declare class StringData implements ExpressionDataInterface {
    readonly value: string;
    constructor(value: string);
    readonly kind = Kind.String;
    primitive: boolean;
    coerceString(): string;
    number(): number;
}
//# sourceMappingURL=string.d.ts.map