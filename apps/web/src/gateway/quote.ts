// W3 的报价.各链的算法在 `chains/` 里,这里只跑查询.
import { useQueries, useQuery } from "@tanstack/react-query";
import { chainOf, fetchFees } from "../chains";
import { baseQuote, type Quote, type QuoteInput, type QueryView } from "../chains/quote";

export function useQuote(input: QuoteInput): Quote {
    const { network } = input;
    const fees = useQuery({
        queryKey: ["chain", network, "fees"],
        queryFn: () => fetchFees(network),
        refetchInterval: 15_000,
        retry: 1,
    });
    // 换链时查询的数量会变,所以用 useQueries,不写成一串 useQuery.
    const plan = chainOf(input.account.chain).quote(input);
    const entries = Object.entries(plan.queries);
    const views = useQueries({
        queries: entries.map(([, query]) => ({
            queryKey: query.key,
            queryFn: query.run,
            enabled: query.enabled ?? true,
            retry: query.retry,
        })),
    });
    const results: Record<string, QueryView> = {};
    entries.forEach(([name], index) => {
        const view = views[index];
        if (view) results[name] = view;
    });
    return plan.build(results, baseQuote(input, fees.isError), fees.data);
}
