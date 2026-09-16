interface Props {
  message: string;
  onConfirm: () => void;
  onCancel: () => void;
  confirmLabel?: string;
  confirmClassName?: string;
}

export default function ConfirmDialog({
  message,
  onConfirm,
  onCancel,
  confirmLabel = "End Connection",
  confirmClassName = "bg-red-600 hover:bg-red-500 text-white",
}: Props) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4">
      <div className="bg-gray-900 border border-gray-700 rounded-2xl p-6 max-w-sm w-full flex flex-col gap-5 shadow-xl">
        <p className="text-white text-center text-sm leading-relaxed">{message}</p>
        <div className="flex flex-col gap-2">
          <button
            onClick={onConfirm}
            className={`w-full font-semibold py-2.5 rounded-xl transition-colors ${confirmClassName}`}
          >
            {confirmLabel}
          </button>
          <button
            onClick={onCancel}
            className="w-full bg-gray-800 hover:bg-gray-700 text-gray-300 font-medium py-2.5 rounded-xl transition-colors"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
