package com.dts.encoder.settings;

public class Probe {
    public static void main(String[] a) throws Exception {
        Model.initialize(Integer.parseInt(a[0]));
        System.out.println("dest " + Model.getLegalDestFormats());
        System.out.println("stream " + Model.getLegalStreamTypes());
        System.out.println("layouts " + Model.getLegalChannelLayouts());
        System.out.println("rates " + Model.getSamplingRateCount() + " bitrates " + Model.getBitRateCount());
        System.out.println("cur " + Model.getDestFormat() + " " + Model.getStreamType() + " " + Model.getSamplingRate() + " " + Model.getChannelLayout() + " " + Model.getBitRate());
    }
}
