import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { cn } from "@/lib/utils";

const Sheet = DialogPrimitive.Root;
const SheetTrigger = DialogPrimitive.Trigger;
const SheetClose = DialogPrimitive.Close;

const SheetOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay> & {
    zIndex?: "base" | "nested" | "alert" | "top";
  }
>(({ className, zIndex = "base", ...props }, ref) => {
  const zIndexMap = {
    base: "z-40",
    nested: "z-50",
    alert: "z-[60]",
    top: "z-[110]",
  };

  return (
    <DialogPrimitive.Overlay
      ref={ref}
      className={cn(
        "fixed inset-0 bg-black/15 data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
        zIndexMap[zIndex],
        className,
      )}
      {...props}
    />
  );
});
SheetOverlay.displayName = DialogPrimitive.Overlay.displayName;

const SheetContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> & {
    zIndex?: "base" | "nested" | "alert" | "top";
    overlayClassName?: string;
  }
>(
  (
    { className, children, zIndex = "base", overlayClassName, ...props },
    ref,
  ) => {
    const zIndexMap = {
      base: "z-40",
      nested: "z-50",
      alert: "z-[60]",
      top: "z-[110]",
    };

    return (
      <DialogPrimitive.Portal>
        <SheetOverlay zIndex={zIndex} className={overlayClassName} />
        <DialogPrimitive.Content
          ref={ref}
          className={cn(
            "fixed right-0 top-0 h-dvh max-h-dvh w-full max-w-md flex flex-col border-l border-border-default bg-background text-foreground shadow-xl overflow-hidden",
            "data-[state=open]:animate-in data-[state=closed]:animate-out",
            "data-[state=closed]:slide-out-to-right data-[state=open]:slide-in-from-right",
            "duration-300",
            zIndexMap[zIndex],
            className,
          )}
          onInteractOutside={(e) => {
            // 表单抽屉：点击遮罩不关闭，防止误触丢失未保存输入
            e.preventDefault();
          }}
          {...props}
        >
          {children}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    );
  },
);
SheetContent.displayName = DialogPrimitive.Content.displayName;

const SheetHeader = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex flex-col space-y-1.5 px-6 py-5 border-b border-border-default bg-muted/20 flex-shrink-0",
      className,
    )}
    {...props}
  />
);
SheetHeader.displayName = "SheetHeader";

const SheetFooter = ({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end sm:items-center px-6 py-5 border-t border-border bg-muted/30 shadow-[0_-4px_12px_-8px_rgba(0,0,0,0.12)] flex-shrink-0",
      className,
    )}
    {...props}
  />
);
SheetFooter.displayName = "SheetFooter";

const SheetTitle = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Title>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Title
    ref={ref}
    className={cn(
      "text-lg font-semibold leading-tight tracking-tight",
      className,
    )}
    {...props}
  />
));
SheetTitle.displayName = DialogPrimitive.Title.displayName;

const SheetDescription = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Description>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Description
    ref={ref}
    className={cn("text-sm text-muted-foreground", className)}
    {...props}
  />
));
SheetDescription.displayName = DialogPrimitive.Description.displayName;

export {
  Sheet,
  SheetTrigger,
  SheetContent,
  SheetHeader,
  SheetFooter,
  SheetTitle,
  SheetDescription,
  SheetClose,
};
