import { createContext, useContext } from 'react';

export type ToastVariant = 'success' | 'danger' | 'info' | 'warning';
export type ShowToast = (message: string, variant: ToastVariant) => void;

/** `showToast` of the one toast the app shows, provided by App (the pages are routed without props). */
export const ToastContext = createContext<ShowToast>(() => undefined);

export const useToast = () => useContext(ToastContext);
