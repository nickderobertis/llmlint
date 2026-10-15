import { ExpressionData, ExpressionDataInterface, Kind, Pair } from "./expressiondata.js";
export declare class Dictionary implements ExpressionDataInterface {
    private keys;
    private v;
    private indexMap;
    constructor(...pairs: Pair[]);
    readonly kind = Kind.Dictionary;
    primitive: boolean;
    coerceString(): string;
    number(): number;
    add(key: string, value: ExpressionData): void;
    get(key: string): ExpressionData | undefined;
    values(): ExpressionData[];
    pairs(): Pair[];
}
export declare function isDictionary(x: ExpressionData): x is Dictionary;
//# sourceMappingURL=dictionary.d.ts.map