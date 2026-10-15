import { ExpressionDataInterface, Kind } from "./expressiondata.js";
export declare class Null implements ExpressionDataInterface {
    readonly kind = Kind.Null;
    primitive: boolean;
    coerceString(): string;
    number(): number;
}
//# sourceMappingURL=null.d.ts.map