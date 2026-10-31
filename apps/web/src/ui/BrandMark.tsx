/** 品牌标志，不随主题变化。32 及以下用粗线版。 */
export function BrandMark({ size }: { size: number }) {
    const small = size <= 32;
    return (
        <svg width={size} height={size} viewBox="0 0 160 160" role="img" aria-label="BitOne" className="block shrink-0">
            <rect width="160" height="160" rx="36" fill="#0B0D11" />
            {small ? (
                <>
                    <path
                        d="M50 40H38A8 8 0 0 0 30 48V112A8 8 0 0 0 38 120H50M110 40H122A8 8 0 0 1 130 48V112A8 8 0 0 1 122 120H110"
                        fill="none"
                        stroke="#EEF1F5"
                        strokeWidth="20"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                    />
                    <rect x="62" y="62" width="36" height="36" rx="8" fill="#1F5EFF" />
                </>
            ) : (
                <>
                    <path
                        d="M56 46H48A10 10 0 0 0 38 56V104A10 10 0 0 0 48 114H56M104 46H112A10 10 0 0 1 122 56V104A10 10 0 0 1 112 114H104"
                        fill="none"
                        stroke="#EEF1F5"
                        strokeWidth="12"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                    />
                    <rect x="66" y="66" width="28" height="28" rx="7" fill="#1F5EFF" />
                </>
            )}
        </svg>
    );
}
