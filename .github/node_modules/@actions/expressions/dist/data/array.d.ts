import { ExpressionData, ExpressionDataInterface, Kind } from "./expressiondata.js";
export declare class Array implements ExpressionDataInterface {
    private v;
    constructor(...data: ExpressionData[]);
    readonly kind = Kind.Array;
    primitive: boolean;
    coerceString(): string;
    number(): number;
    add(value: ExpressionData): void;
    get(index: number): ExpressionData;
    values(): ExpressionData[];
}
//# sourceMappingURL=array.d.ts.map