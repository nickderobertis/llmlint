import { Dictionary } from "./dictionary.js";
import { Null } from "./null.js";
import { Array } from "./array.js";
import { StringData } from "./string.js";
import { NumberData } from "./number.js";
import { BooleanData } from "./boolean.js";
export declare enum Kind {
    String = 0,
    Array = 1,
    Dictionary = 2,
    Boolean = 3,
    Number = 4,
    CaseSensitiveDictionary = 5,
    Null = 6
}
export declare function kindStr(k: Kind): string;
export interface ExpressionDataInterface {
    kind: Kind;
    primitive: boolean;
    coerceString(): string;
    number(): number;
}
export type ExpressionData = Array | Dictionary | StringData | BooleanData | NumberData | Null;
export type Pair = {
    key: string;
    value: ExpressionData;
};
//# sourceMappingURL=expressiondata.d.ts.map