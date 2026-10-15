import { Dictionary } from "../data/dictionary.js";
import { ExpressionData, Pair } from "../data/expressiondata.js";
export type DescriptionPair = Pair & {
    description?: string;
};
export declare function isDescriptionDictionary(x: ExpressionData): x is DescriptionDictionary;
export declare class DescriptionDictionary extends Dictionary {
    private readonly descriptions;
    complete: boolean;
    constructor(...pairs: DescriptionPair[]);
    add(key: string, value: ExpressionData, description?: string): void;
    pairs(): DescriptionPair[];
    getDescription(key: string): string | undefined;
}
//# sourceMappingURL=descriptionDictionary.d.ts.map